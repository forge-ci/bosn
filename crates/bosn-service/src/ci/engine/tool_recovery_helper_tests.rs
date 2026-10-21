//! #508: real journal transitions over a fault-injected Docker command boundary.
use super::*;
use crate::ci::{
    engine::cache_usage::helper::recovery_cleanup_tests::seed_source,
    lifecycle::tests::{OWNER, with_registry},
};
use bosn_registry::{Registry, cache_helper::CacheHelperState};
use kernal_api::platform::fs::TemporaryDirectory;

fn backend(
    record: &ActEngineRecord,
    state: &std::path::Path,
    mode: &str,
) -> (TemporaryDirectory, DockerActBackend) {
    let dir = TemporaryDirectory::new().unwrap();
    let frozen = record.tool_recovery.as_ref().unwrap();
    let source_labels = bosn_core::ResourceLabels::new(
        OWNER,
        bosn_core::ResourceKind::Volume,
        "act-storage",
        &record.intent.run_id,
        bosn_core::Scope::Spec,
        &record.intent.run_id,
        &record.intent.created_at.to_string(),
        Some(bosn_core::Retention::Warm),
    )
    .unwrap()
    .to_map();
    let volume = |name: &str, labels| {
        serde_json::json!([{
            "Name":name,"Driver":"local","Scope":"local","Options":null,"Labels":labels
        }])
    };
    std::fs::write(
        dir.path().join("source.json"),
        volume(
            &frozen.source_volume,
            serde_json::to_value(source_labels).unwrap(),
        )
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("cache.json"),
        volume(
            &frozen.cache_volume,
            serde_json::to_value(crate::act_engine::cache_volume_labels(OWNER, 1.0).unwrap())
                .unwrap(),
        )
        .to_string(),
    )
    .unwrap();
    let data = serde_json::json!({"mode":mode,"source":frozen.source_volume,"cache":frozen.cache_volume,
        "run":record.intent.run_id,"engine":frozen.engine_id,"engine_name":record.intent.engine_name(),
        "db":state.join("registry.sqlite3"),"pin":RecoveryPin::from_intent(frozen).unwrap()});
    std::fs::write(dir.path().join("input.json"), data.to_string()).unwrap();
    let script = dir.path().join("docker.py");
    std::fs::write(&script, DOCKER).unwrap();
    let backend = DockerActBackend::new(bosn_engine::DockerEngine::synthetic_for_test(
        "python3",
        [script.into_os_string(), dir.path().as_os_str().to_owned()],
    ));
    (dir, backend)
}

fn commands(directory: &std::path::Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(directory.join("commands"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn recovery_helper_publication_and_cleanup_preserve_durable_ordering() {
    for mode in [
        "ok",
        "partial",
        "bad-pin",
        "trailing",
        "failed-start",
        "source-changed",
        "wrong-id",
        "lost-create",
        "foreign-command",
        "cleanup-denied",
        "lost-remove",
        "engine-present",
        "engine-ambiguous",
        "cleanup-mixed",
        "helper-ambiguous",
        "oversized",
    ] {
        with_registry(|actor, state| async move {
            let record = seed_source(&actor, false).await;
            let (dir, backend) = backend(&record, &state, mode);
            let result = backend
                .recover_tool_with_helper(&actor, &record.intent.run_id, OWNER)
                .await;
            assert_eq!(result.is_ok(), mode == "ok", "{mode}: {result:?}");
            let reader = Registry::open_read_only(state.join("registry.sqlite3")).unwrap();
            let after = reader.act_engine(&record.intent.run_id).unwrap().unwrap();
            assert_eq!(
                after.tool_recovery_completion.is_some(),
                mode == "ok",
                "{mode}"
            );
            assert!(after.tool_recovery_source_disposal_at.is_none());
            assert!(after.retains_tool_recovery_source(crate::ci::lifecycle::now_seconds()));
            let commands = commands(dir.path());
            assert!(
                !commands
                    .iter()
                    .any(|a| a.first().is_some_and(|s| s == "volume")
                        && a.get(1).is_some_and(|s| s == "rm"))
            );
            if matches!(mode, "engine-present" | "engine-ambiguous") {
                return;
            }
            let nonce = commands
                .iter()
                .find(|a| a[0] == "create")
                .unwrap()
                .windows(2)
                .find(|p| p[0] == "--label" && p[1].starts_with("io.bosn.cache.recovery="))
                .unwrap()[1]
                .split_once('=')
                .unwrap()
                .1
                .to_owned();
            let h = reader.cache_helper(&nonce).unwrap().unwrap();
            let pending = matches!(
                mode,
                "foreign-command"
                    | "cleanup-denied"
                    | "lost-remove"
                    | "cleanup-mixed"
                    | "helper-ambiguous"
            );
            assert_eq!(
                h.state == CacheHelperState::Removed,
                !pending,
                "{mode}: {h:?}"
            );
            assert_ne!(
                h.container_id.as_deref(),
                Some("9999999999999999999999999999999999999999999999999999999999999999")
            );
            if matches!(
                mode,
                "source-changed" | "wrong-id" | "lost-create" | "foreign-command"
            ) {
                assert!(!dir.path().join("started").exists(), "{mode}");
            }
            if mode == "ok" {
                assert!(
                    backend
                        .recover_tool_with_helper(&actor, &record.intent.run_id, OWNER)
                        .await
                        .is_ok()
                );
                assert_eq!(commands.iter().filter(|a| a[0] == "start").count(), 1);
                assert!(
                    after
                        .tool_recovery_completion
                        .unwrap()
                        .reference_released_at
                        .is_none()
                );
            }
        });
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveFixture {
    cache: String,
    generation: String,
}

#[test]
#[ignore = "requires explicitly provisioned private overlay volumes and pinned offline act archive"]
fn actual_docker_recovery_helper_persists_publication_after_pid1_teardown() {
    let path = std::env::var("BOSN_RECOVERY_LIVE_FIXTURE").unwrap();
    let fixture: LiveFixture = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(fixture.cache.starts_with("bosn693-live-publication-"));
    with_registry(|actor, state| async move {
        let record =
            crate::ci::engine::cache_usage::helper::recovery_cleanup_tests::seed_source_with(
                &actor,
                false,
                &fixture.generation,
                &fixture.cache,
            )
            .await;
        let frozen = record.tool_recovery.as_ref().unwrap();
        let backend = DockerActBackend::default();
        let act = act_artifact("amd64").unwrap();
        let encoded = RecoveryPin::from_intent(frozen).unwrap().encoded().unwrap();
        let script = format!(
            "{}; printf '%s' '{encoded}' >{WORK}/intent.json; exec {WORK}/bin/act --cache-server-path {ENGINE_CACHE}/toolstore-v1 cache tool-recovery reserve --record {WORK}/intent.json --max-bytes {} --apply",
            offline_act_script(act, &format!("{WORK}/bin")),
            64 << 20
        );
        let name = format!("{}-reserve", fixture.cache);
        backend
            .checked(
                "live fixture reservation",
                owned(&[
                    "run",
                    "--rm",
                    bosn_engine::LOW_CPU_PRIORITY_ARG,
                    "--name",
                    &name,
                    "--label",
                    "io.bosn.task=693",
                    "--label",
                    "io.bosn.fixture=live-publication",
                    "--network",
                    "none",
                    "--read-only",
                    "--cap-drop",
                    "ALL",
                    "--tmpfs",
                    "/tmp:exec,size=64m",
                    "--mount",
                    &format!("type=volume,source={},target={ENGINE_CACHE}", fixture.cache),
                    "--entrypoint",
                    "/bin/sh",
                    &engine_image(),
                    "-ec",
                    &script,
                ]),
                CONTROL_DEADLINE,
            )
            .await
            .unwrap();
        backend.confirm_measurement_absent(&name).await.unwrap();
        backend
            .recover_tool_with_helper(&actor, &record.intent.run_id, OWNER)
            .await
            .unwrap();
        let reader = Registry::open_read_only(state.join("registry.sqlite3")).unwrap();
        let after = reader.act_engine(&record.intent.run_id).unwrap().unwrap();
        let done = after.tool_recovery_completion.as_ref().unwrap();
        assert!(done.reference_released_at.is_none());
        assert!(after.retains_tool_recovery_source(crate::ci::lifecycle::now_seconds()));
        assert!(after.tool_recovery_source_disposal_at.is_none());
        backend.verify_recovery_source(&after, OWNER).await.unwrap();
        let helper: bosn_registry::cache_helper::CacheHelperRecord = reader
            .events(0, 64)
            .unwrap()
            .items
            .iter()
            .rev()
            .find(|event| event.kind.starts_with("ci.cache-helper.v1:"))
            .map(|event| serde_json::from_str(&event.detail).unwrap())
            .unwrap();
        assert_eq!(helper.state, CacheHelperState::Removed);
        backend
            .confirm_measurement_absent(helper.container_id.as_ref().unwrap())
            .await
            .unwrap();
        assert!(
            backend
                .recover_tool_with_helper(&actor, &record.intent.run_id, OWNER)
                .await
                .is_ok()
        );
        std::fs::write(
            format!("{path}.result.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "record":after,"helper":helper,"fixture_reservation":name,
                "pin":RecoveryPin::from_intent(frozen).unwrap(),
            }))
            .unwrap(),
        )
        .unwrap();
    });
}

const DOCKER: &str = r#"
import sys,json,sqlite3,datetime
from pathlib import Path
root=Path(sys.argv[1]); args=sys.argv[2:]; data=json.loads((root/'input.json').read_text());mode=data['mode']
with (root/'commands').open('a') as out: out.write(json.dumps(args)+'\n')
def rows(prefix):
    with sqlite3.connect(data['db']) as db:
        return [(n,json.loads(s)) for n,s in db.execute('SELECT id,detail FROM events WHERE kind LIKE ? ORDER BY id',(prefix+'%',))]
def engine(): return rows('act.engine.v1:')[-1][1]
def helper(): return rows('ci.cache-helper.v1:')[-1][1]
def absent(): print('Error: No such container: '+args[-1],file=sys.stderr);sys.exit(1)
def flag(key): return args[args.index(key)+1]
if args[:2]==['image','inspect']:
    print('amd64' if '{{.Architecture}}' in args else 'sha256:'+'a'*64)
elif args[:2]==['volume','inspect']:
    assert args[-1] in [data['source'],data['cache']]
    if mode=='source-changed' and (root/'created').exists() and args[-1]==data['source']:
        row=json.loads((root/'source.json').read_text());row[0]['Labels']={};print(json.dumps(row))
    else: print((root/('source.json' if args[-1]==data['source'] else 'cache.json')).read_text())
elif args[0]=='create':
    h=helper();assert h['state']=='pending' and h.get('container_id') is None
    assert engine().get('tool_recovery_completion') is None
    assert '--init=false' in args and '--privileged' in args and '--cpu-shares=256' in args
    assert args[-2]=='-ec' and 'exec /tmp/bosn-recovery/bin/act ' in args[-1]
    assert 'wget' not in args[-1] and 'docker exec' not in args[-1]
    labels=dict(args[i+1].split('=',1) for i,a in enumerate(args) if a=='--label')
    mounts=[]
    for i,a in enumerate(args):
        if a=='--mount':
            v=dict(x.split('=',1) for x in args[i+1].split(','))
            mounts.append(dict(Type=v['type'],Name=v['source'],Destination=v['target'],RW=True,Driver='local'))
    row=dict(Id='1'*64,Name='/'+flag('--name'),Config=dict(Image=args[-3],Labels=labels,Entrypoint=['/bin/sh'],Cmd=args[-2:]),
        HostConfig=dict(ReadonlyRootfs=True,Privileged=True,NetworkMode='none',Memory=536870912,NanoCpus=2000000000,
        PidsLimit=128,CpuShares=256,Tmpfs={'/tmp':'exec,size=64m'},Init=False,PidMode='',AutoRemove=True,
        RestartPolicy=dict(Name='no',MaximumRetryCount=0)),Mounts=mounts)
    if mode=='foreign-command': row['Config']['Cmd']=['-ec','sleep 600']
    (root/'created').write_text(json.dumps([row]))
    if mode=='lost-create': print('reply lost',file=sys.stderr);sys.exit(1)
    print(('9' if mode=='wrong-id' else '1')*64)
elif args[:2]==['container','inspect']:
    if args[-1] in [data['engine'],data['engine_name']]:
        if mode=='engine-present': print('[{}]')
        else:
            if mode=='engine-ambiguous': print('permission denied',file=sys.stderr)
            absent()
    elif mode=='helper-ambiguous' and (root/'started').exists():
        print('permission denied',file=sys.stderr);absent()
    elif (root/'removed').exists() or not (root/'created').exists(): absent()
    else: print((root/'created').read_text())
elif args[0]=='start':
    assert args==['start','--attach','1'*64]
    assert helper()['container_id']=='1'*64
    assert engine().get('tool_recovery_completion') is None
    (root/'started').write_text('yes')
    at=datetime.datetime.now(datetime.timezone.utc).isoformat().replace('+00:00','Z')
    report=dict(schema_version=1,root='/bosn/cache/toolstore-v1',pin=data['pin'],generation='d'*64,
        completed_at=at,completed=True,partial=False)
    if mode=='partial': report['partial']=True
    if mode=='bad-pin': report['pin']['owner']='f'*64
    if mode=='trailing': print('diagnostic on stdout')
    if mode=='oversized': print(' '*4097,end='')
    print(json.dumps(report))
    if mode=='failed-start':sys.exit(1)
elif args[0]=='rm':
    assert args==['rm','-f','-v','1'*64]
    assert engine().get('tool_recovery_completion') is None
    if mode in ['cleanup-denied','cleanup-mixed']:
        if mode=='cleanup-mixed': print('Error: No such container: '+'1'*64,file=sys.stderr)
        print('permission denied',file=sys.stderr);sys.exit(1)
    (root/'removed').write_text('yes')
    if mode=='lost-remove': print('reply lost',file=sys.stderr);sys.exit(1)
else: raise AssertionError(args)
"#;
