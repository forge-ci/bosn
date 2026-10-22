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
        Some(bosn_core::Retention::Pinned),
    )
    .unwrap()
    .to_map();
    let volume = |name: &str, labels| {
        serde_json::json!([{
            "Name":name,"Driver":"local","Scope":"local","Options":null,"Labels":labels,
            "Mountpoint":format!("/var/lib/docker/volumes/{name}/_data")
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
        "db":state.join("registry.sqlite3"),"release":record.tool_recovery_completion.is_some() && record.tool_recovery_source_disposal_at.is_none(),"disposal":record.tool_recovery_source_disposal_at.is_some(),"pin":RecoveryPin::from_intent(frozen).unwrap()});
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
fn reference_release_helper_requires_completion_and_never_contacts_source() {
    for mode in [
        "ok",
        "absent-retry",
        "partial",
        "bad-pin",
        "not-absent",
        "trailing",
        "oversized",
        "failed-start",
        "wrong-id",
        "lost-create",
        "foreign-command",
        "cleanup-denied",
        "lost-remove",
        "cleanup-mixed",
        "helper-ambiguous",
        "cache-changed",
    ] {
        with_registry(|actor, state| async move {
            let record = seed_source(&actor, false).await;
            let (dir, before) = backend(&record, &state, mode);
            assert!(
                before
                    .release_tool_with_helper(&actor, &record.intent.run_id, OWNER)
                    .await
                    .is_err()
            );
            assert!(
                !dir.path().join("commands").exists(),
                "release before durable completion contacted Docker"
            );
            let record =
                complete_source(&actor, &record, crate::ci::lifecycle::now_seconds()).await;
            let (dir, backend) = backend(&record, &state, mode);
            let result = backend
                .release_tool_with_helper(&actor, &record.intent.run_id, OWNER)
                .await;
            let success = matches!(mode, "ok" | "absent-retry");
            assert_eq!(result.is_ok(), success, "{mode}: {result:?}");
            let reader = Registry::open_read_only(state.join("registry.sqlite3")).unwrap();
            assert_release_state(&reader, &record.intent.run_id, success);
            let commands = commands(dir.path());
            assert_source_free(&commands, &record);
            let helper = last_helper(&reader);
            let pending = matches!(
                mode,
                "foreign-command"
                    | "cleanup-denied"
                    | "lost-remove"
                    | "cleanup-mixed"
                    | "helper-ambiguous"
            );
            assert_eq!(
                helper.state == CacheHelperState::Removed,
                !pending,
                "{mode}: {helper:?}"
            );
            if mode == "lost-remove" {
                assert!(
                    backend
                        .retry_measurements(&actor, OWNER, None)
                        .await
                        .unwrap()
                        .removed
                        .is_some()
                );
                assert!(
                    reader
                        .act_engine(&record.intent.run_id)
                        .unwrap()
                        .unwrap()
                        .tool_recovery_completion
                        .unwrap()
                        .reference_released_at
                        .is_none()
                );
            }
            if success {
                let before = std::fs::read_to_string(dir.path().join("commands")).unwrap();
                backend
                    .release_tool_with_helper(&actor, &record.intent.run_id, OWNER)
                    .await
                    .unwrap();
                assert_eq!(
                    std::fs::read_to_string(dir.path().join("commands")).unwrap(),
                    before,
                    "acknowledged release retried Docker"
                );
            }
        });
    }
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
        "source-path-changed",
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
            assert_publication_helper_journal(&commands, &reader, mode);
            if matches!(
                mode,
                "source-changed"
                    | "source-path-changed"
                    | "wrong-id"
                    | "lost-create"
                    | "foreign-command"
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

fn assert_publication_helper_journal(
    commands: &[Vec<String>],
    reader: &bosn_registry::ReadOnlyRegistry,
    mode: &str,
) {
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
        "foreign-command" | "cleanup-denied" | "lost-remove" | "cleanup-mixed" | "helper-ambiguous"
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
}

#[test]
fn recovery_worker_skips_expired_publication_and_advances_its_cursor_on_failure() {
    for mode in ["ok", "partial"] {
        with_registry(|actor, state| async move {
            let record=crate::ci::engine::cache_usage::helper::recovery_cleanup_tests::seed_source_for_lifetime(
                &actor,true,&"c".repeat(64),"bosn-ci-cache-test",5).await;
            let (dir, before) = backend(&record, &state, mode);
            let tick = before
                .retry_tool_recovery(&actor, OWNER, None)
                .await
                .unwrap();
            assert!(
                tick.advanced.is_none() && tick.deferred.is_none() && tick.next_run_id.is_none()
            );
            assert!(
                !dir.path().join("commands").exists(),
                "expired source was republished"
            );
            actor
                .act_registry(ActRegistryCommand::ToolRecoveryPublished {
                    run: record.intent.run_id.clone(),
                    proof: bosn_registry::act::ActToolRecoveryPublicationProof {
                        intent: record.tool_recovery.clone().unwrap(),
                        generation: "d".repeat(64),
                    },
                    at: record.updated_at + 0.1,
                })
                .await
                .unwrap();
            let ActRegistryReply::Record(Some(record)) = actor
                .act_registry(ActRegistryCommand::Get {
                    run: record.intent.run_id.clone(),
                })
                .await
                .unwrap()
            else {
                panic!("completed record missing");
            };
            let (dir, backend) = backend(&record, &state, mode);
            let tick = backend
                .retry_tool_recovery(&actor, OWNER, None)
                .await
                .unwrap();
            assert_eq!(
                tick.next_run_id.as_deref(),
                Some(record.intent.run_id.as_str())
            );
            assert_eq!(tick.advanced.is_some(), mode == "ok");
            assert_eq!(tick.deferred.is_some(), mode != "ok");
            let previous = commands(dir.path());
            let empty = backend
                .retry_tool_recovery(&actor, OWNER, tick.next_run_id)
                .await
                .unwrap();
            assert!(
                empty.next_run_id.is_none() && empty.advanced.is_none() && empty.deferred.is_none()
            );
            assert_eq!(
                commands(dir.path()),
                previous,
                "cursor repeated the failed front record"
            );
        });
    }
}

fn assert_source_free(commands: &[Vec<String>], record: &ActEngineRecord) {
    let frozen = record.tool_recovery.as_ref().unwrap();
    assert!(
        !commands
            .iter()
            .any(|a| a.iter().any(|s| s.contains(&frozen.source_volume)
                || s.contains(&frozen.engine_id)
                || s.contains(&record.intent.engine_name())))
    );
    assert!(
        !commands.iter().any(
            |a| a.first().is_some_and(|s| s == "volume") && a.get(1).is_some_and(|s| s == "rm")
        )
    );
}

async fn complete_source(
    actor: &RegistryActor,
    record: &ActEngineRecord,
    at: f64,
) -> ActEngineRecord {
    actor
        .act_registry(ActRegistryCommand::ToolRecoveryPublished {
            run: record.intent.run_id.clone(),
            proof: bosn_registry::act::ActToolRecoveryPublicationProof {
                intent: record.tool_recovery.clone().unwrap(),
                generation: "d".repeat(64),
            },
            at,
        })
        .await
        .unwrap();
    let ActRegistryReply::Record(Some(record)) = actor
        .act_registry(ActRegistryCommand::Get {
            run: record.intent.run_id.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("completed record missing");
    };
    *record
}

fn assert_release_state(reader: &bosn_registry::ReadOnlyRegistry, run: &str, success: bool) {
    let saved = reader.act_engine(run).unwrap().unwrap();
    assert_eq!(
        saved
            .tool_recovery_completion
            .as_ref()
            .unwrap()
            .reference_released_at
            .is_some(),
        success
    );
    assert_eq!(
        saved.retains_tool_recovery_source(crate::ci::lifecycle::now_seconds()),
        !success
    );
    assert!(saved.tool_recovery_source_disposal_at.is_none());
}

async fn dispose_expired_live_source(
    actor: &RegistryActor,
    backend: &DockerActBackend,
    record: &ActEngineRecord,
) {
    let frozen = record.tool_recovery.as_ref().unwrap();
    let wait =
        (frozen.expires_at_seconds as f64 + 0.01 - crate::ci::lifecycle::now_seconds()).max(0.0);
    kernal_api::async_engine::sleep(Duration::from_secs_f64(wait)).await;
    let disposal = crate::act_engine::remove_storage_volume(actor, &backend.docker, record, OWNER)
        .await
        .unwrap();
    actor
        .act_registry(ActRegistryCommand::Finalize {
            run: record.intent.run_id.clone(),
            proof: bosn_registry::act::ActEngineRemovalProof {
                name: record.intent.engine_name(),
                engine_id: Some(frozen.engine_id.clone()),
                storage_volume: record.intent.storage_volume_name(),
            },
            at: crate::ci::lifecycle::now_seconds().max(disposal),
        })
        .await
        .unwrap();
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveFixture {
    cache: String,
    generation: String,
}

pub(in crate::ci::engine) async fn reserve_live_reference(
    backend: &DockerActBackend,
    record: &ActEngineRecord,
) -> String {
    let frozen = record.tool_recovery.as_ref().unwrap();
    let act = act_artifact("amd64").unwrap();
    let encoded = RecoveryPin::from_intent(frozen).unwrap().encoded().unwrap();
    let script = format!(
        "{}; printf '%s' '{encoded}' >{WORK}/intent.json; exec {WORK}/bin/act --cache-server-path {ENGINE_CACHE}/toolstore-v1 cache tool-recovery reserve --record {WORK}/intent.json --max-bytes {} --apply",
        offline_act_script(act, &format!("{WORK}/bin")),
        64 << 20
    );
    let name = format!("{}-reserve", frozen.cache_volume);
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
                &format!(
                    "type=volume,source={},target={ENGINE_CACHE}",
                    frozen.cache_volume
                ),
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
    name
}

fn last_helper(
    reader: &bosn_registry::ReadOnlyRegistry,
) -> bosn_registry::cache_helper::CacheHelperRecord {
    reader
        .events(0, 64)
        .unwrap()
        .items
        .iter()
        .rev()
        .find(|event| event.kind.starts_with("ci.cache-helper.v1:"))
        .map(|event| serde_json::from_str(&event.detail).unwrap())
        .unwrap()
}

#[test]
#[ignore = "requires explicitly provisioned private overlay volumes and pinned offline act archive"]
fn actual_docker_recovery_helper_persists_publication_after_pid1_teardown() {
    let path = std::env::var("BOSN_RECOVERY_LIVE_FIXTURE").unwrap();
    let fixture: LiveFixture = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(fixture.cache.starts_with("bosn693-live-publication-"));
    with_registry(|actor, state| async move {
        let record =
            crate::ci::engine::cache_usage::helper::recovery_cleanup_tests::seed_source_for_lifetime(
                &actor, false, &fixture.generation, &fixture.cache, 20,
            )
            .await;
        let frozen = record.tool_recovery.as_ref().unwrap();
        let backend = DockerActBackend::default();
        let name = reserve_live_reference(&backend, &record).await;
        let publication = backend
            .retry_tool_recovery(&actor, OWNER, None)
            .await
            .unwrap();
        assert_eq!(
            publication.advanced.as_deref(),
            Some(record.intent.run_id.as_str()),
            "{publication:?}"
        );
        assert!(publication.deferred.is_none());
        let reader = Registry::open_read_only(state.join("registry.sqlite3")).unwrap();
        let after = reader.act_engine(&record.intent.run_id).unwrap().unwrap();
        let done = after.tool_recovery_completion.as_ref().unwrap();
        assert!(done.reference_released_at.is_none());
        assert!(after.retains_tool_recovery_source(crate::ci::lifecycle::now_seconds()));
        assert!(after.tool_recovery_source_disposal_at.is_none());
        backend.verify_recovery_source(&after, OWNER).await.unwrap();
        let helper = last_helper(&reader);
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
        // Exercise real finite expiry and the production source-removal gate,
        // then reconcile reference release with the source physically absent.
        dispose_expired_live_source(&actor, &backend, &after).await;
        let disposal_helper = assert_disposal_removed(&reader, &backend).await;
        let release = backend
            .retry_tool_recovery(&actor, OWNER, None)
            .await
            .unwrap();
        assert_eq!(
            release.advanced.as_deref(),
            Some(record.intent.run_id.as_str()),
            "{release:?}"
        );
        assert!(release.deferred.is_none());
        let released = reader.act_engine(&record.intent.run_id).unwrap().unwrap();
        assert_eq!(released.state, bosn_registry::act::ActEngineState::Terminal);
        assert!(
            released
                .tool_recovery_completion
                .as_ref()
                .unwrap()
                .reference_released_at
                .is_some()
        );
        let release_helper = last_helper(&reader);
        assert_eq!(release_helper.state, CacheHelperState::Removed);
        assert!(matches!(
            release_helper.intent.role,
            Some(CacheHelperRole::ToolRecoveryReleaseV1 { .. })
        ));
        backend
            .confirm_measurement_absent(release_helper.container_id.as_ref().unwrap())
            .await
            .unwrap();
        backend
            .release_tool_with_helper(&actor, &record.intent.run_id, OWNER)
            .await
            .unwrap();
        std::fs::write(
            format!("{path}.result.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "publication_record":after,"record":released,"helper":helper,"disposal_helper":disposal_helper,"release_helper":release_helper,"fixture_reservation":name,
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
root=Path(sys.argv[1]); args=sys.argv[2:]; data=json.loads((root/'input.json').read_text());mode=data['mode'];release=data['release'];disposal=data['disposal'];sourcefree=release or disposal
with (root/'commands').open('a') as out: out.write(json.dumps(args)+'\n')
def rows(prefix):
    with sqlite3.connect(data['db']) as db:
        return [(n,json.loads(s)) for n,s in db.execute('SELECT id,detail FROM events WHERE kind LIKE ? ORDER BY id',(prefix+'%',))]
def engine(): return rows('act.engine.v1:')[-1][1]
def helper(): return rows('ci.cache-helper.v1:')[-1][1]
def absent(): print('Error: No such container: '+args[-1],file=sys.stderr);sys.exit(1)
def flag(key): return args[args.index(key)+1]
def ready():
    done=engine().get('tool_recovery_completion')
    if disposal: assert engine().get('tool_recovery_source_disposal_at') is not None
    elif release: assert done is not None and done.get('reference_released_at') is None
    else: assert done is None
if args[:2]==['image','inspect']:
    print('amd64' if '{{.Architecture}}' in args else 'sha256:'+'a'*64)
elif args[:2]==['volume','inspect']:
    assert args[-1] in [data['source'],data['cache']]
    if release: assert args[-1]==data['cache']
    if mode=='cache-changed' and (root/'created').exists():
        row=json.loads((root/'cache.json').read_text());row[0]['Labels']={};print(json.dumps(row))
    elif mode=='source-path-changed' and (root/'created').exists() and args[-1]==data['source']:
        row=json.loads((root/'source.json').read_text());row[0]['Mountpoint']='/var/lib/docker/volumes/replaced/_data';print(json.dumps(row))
    elif mode=='source-changed' and (root/'created').exists() and args[-1]==data['source']:
        row=json.loads((root/'source.json').read_text());row[0]['Labels']={};print(json.dumps(row))
    else: print((root/('source.json' if args[-1]==data['source'] else 'cache.json')).read_text())
elif args[0]=='create':
    h=helper();assert h['state']=='pending' and h.get('container_id') is None
    ready()
    assert '--init=false' in args and '--cpu-shares=256' in args
    if (root/'removed').exists(): (root/'removed').rename(root/'previous-removed')
    assert ('--privileged' in args)==(not sourcefree)
    if sourcefree: assert flag('--cap-drop')=='ALL' and (' cache tool-recovery '+('dispose-source' if disposal else 'release')+' ') in args[-1] and '--source-root' not in args[-1]
    assert args[-2]=='-ec' and 'exec /tmp/bosn-recovery/bin/act ' in args[-1]
    assert 'wget' not in args[-1] and 'docker exec' not in args[-1]
    labels=dict(args[i+1].split('=',1) for i,a in enumerate(args) if a=='--label')
    mounts=[]
    for i,a in enumerate(args):
        if a=='--mount':
            v=dict((x.split('=',1)+[''])[:2] for x in args[i+1].split(','))
            mount=dict(Type=v['type'],Destination=v['target'],RW='readonly' not in v)
            if v['type']=='volume': mount.update(Name=v['source'],Driver='local')
            else: mount.update(Source=v['source'])
            mounts.append(mount)
    row=dict(Id='1'*64,Name='/'+flag('--name'),Config=dict(Image=args[-3],Labels=labels,Entrypoint=['/bin/sh'],Cmd=args[-2:]),
        HostConfig=dict(ReadonlyRootfs=True,Privileged=not sourcefree,CapDrop=['ALL'] if sourcefree else None,NetworkMode='none',Memory=536870912,NanoCpus=2000000000,
        PidsLimit=128,CpuShares=256,Tmpfs=({'/tmp':'exec,size=64m','/var/lib/docker':'ro,noexec,size=1m'} if sourcefree else {'/tmp':'exec,size=64m'}),Init=False,PidMode='',AutoRemove=True,
        RestartPolicy=dict(Name='no',MaximumRetryCount=0)),Mounts=mounts)
    if mode=='foreign-command': row['Config']['Cmd']=['-ec','sleep 600']
    (root/'created').write_text(json.dumps([row]))
    if mode=='lost-create': print('reply lost',file=sys.stderr);sys.exit(1)
    print(('9' if mode=='wrong-id' else '1')*64)
elif args[:2]==['container','inspect']:
    if args[-1] in [data['engine'],data['engine_name']]:
        assert not release,'release must not inspect or require its former source engine'
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
    ready()
    (root/'started').write_text('yes')
    at=datetime.datetime.now(datetime.timezone.utc).isoformat().replace('+00:00','Z')
    report=dict(schema_version=1,root='/bosn/cache/toolstore-v1',pin=data['pin'],generation='d'*64,
        completed_at=at,completed=True,partial=False)
    if release: report=dict(schema_version=1,root='/bosn/cache/toolstore-v1',pin=data['pin'],removed=mode!='absent-retry',absent=mode!='not-absent',partial=False)
    if disposal:
        (root/'fenced').write_text('durable producer fence')
        report=dict(schema_version=1,root='/bosn/cache/toolstore-v1',pin=data['pin'],disposed=mode!='not-disposed',partial=False)
    if mode=='wrong-root': report['root']='/foreign'
    if mode=='wrong-schema': report['schema_version']=2
    if mode=='unknown': report['unknown']=True
    if mode=='report-error': report['error']='directory sync failed'
    if mode=='partial': report['partial']=True
    if mode=='bad-pin': report['pin']['owner']='f'*64
    if mode=='trailing': print('diagnostic on stdout')
    if mode=='oversized': print(' '*4097,end='')
    print(json.dumps(report))
    if mode=='failed-start':sys.exit(1)
elif args[:2]==['volume','ls']:
    if (root/'source.json').exists(): print(data['source'])
elif args[:2]==['volume','rm']:
    assert disposal and args==['volume','rm',data['source']]
    assert (root/'fenced').exists() and (root/'removed').exists()
    (root/'source.json').rename(root/'source-removed.json')
    if mode=='lost-volume-remove': sys.exit(1)
elif args[0]=='rm':
    assert args==['rm','-f','-v','1'*64]
    ready()
    if mode in ['cleanup-denied','cleanup-mixed']:
        if mode=='cleanup-mixed': print('Error: No such container: '+'1'*64,file=sys.stderr)
        print('permission denied',file=sys.stderr);sys.exit(1)
    (root/'removed').write_text('yes')
    if mode=='lost-remove': print('reply lost',file=sys.stderr);sys.exit(1)
else: raise AssertionError(args)
"#;

#[test]
fn publication_source_attachment_never_auto_creates_a_volume() {
    with_registry(|actor, state| async move {
        let record = seed_source(&actor, false).await;
        let (dir, backend) = backend(&record, &state, "ok");
        backend
            .recover_tool_with_helper(&actor, &record.intent.run_id, OWNER)
            .await
            .unwrap();
        let calls = commands(dir.path());
        let create = calls.iter().find(|args| args[0] == "create").unwrap();
        let frozen = record.tool_recovery.as_ref().unwrap();
        let expected = format!(
            "type=bind,source=/var/lib/docker/volumes/{}/_data,target=/var/lib/docker",
            frozen.source_volume
        );
        assert!(
            create.iter().any(|argument| argument == &expected),
            "publication source can be auto-created by Docker instead of refusing absence: {create:?}"
        );
    });
}

#[test]
fn publication_refuses_unsafe_mountpoints_before_journaling_or_create() {
    for path in [
        None,
        Some("relative"),
        Some("/"),
        Some("/volumes/../private"),
        Some("/volumes/a,b/_data"),
        Some("/volumes//data"),
    ] {
        with_registry(|actor, state| async move {
            let record = seed_source(&actor, false).await;
            let (dir, backend) = backend(&record, &state, "ok");
            let file = dir.path().join("source.json");
            let mut document: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
            document[0]["Mountpoint"] = serde_json::to_value(path).unwrap();
            std::fs::write(file, document.to_string()).unwrap();
            assert!(
                backend
                    .recover_tool_with_helper(&actor, &record.intent.run_id, OWNER)
                    .await
                    .is_err()
            );
            assert!(!commands(dir.path()).iter().any(|args| args[0] == "create"));
            let reader = Registry::open_read_only(state.join("registry.sqlite3")).unwrap();
            assert!(
                reader
                    .events(0, 64)
                    .unwrap()
                    .items
                    .iter()
                    .all(|event| !event.kind.starts_with("ci.cache-helper.v1:"))
            );
        });
    }
}

#[path = "tool_recovery_disposal_tests.rs"]
mod disposal_tests;

async fn assert_disposal_removed(
    reader: &bosn_registry::ReadOnlyRegistry,
    backend: &DockerActBackend,
) -> bosn_registry::cache_helper::CacheHelperRecord {
    let helper = last_helper(reader);
    assert!(matches!(
        helper.intent.role,
        Some(CacheHelperRole::ToolRecoveryDisposalV1 { .. })
    ));
    assert_eq!(helper.state, CacheHelperState::Removed);
    backend
        .confirm_measurement_absent(helper.container_id.as_ref().unwrap())
        .await
        .unwrap();
    helper
}
