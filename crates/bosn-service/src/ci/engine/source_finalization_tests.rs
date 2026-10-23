//! Real replacement-registry journal against a fault-injected Docker boundary.
use super::*;
use crate::ci::{
    engine::{
        DockerActBackend,
        source_context::{SourceContext, tests::document},
    },
    lifecycle::tests::{Daemon, OWNER, with_registry},
};
use bosn_registry::{Registry, cache_helper::CacheHelperState};
use kernal_api::platform::fs::TemporaryDirectory;

const REPLACEMENT: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
const CONTEXT: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

fn boundary(
    context: &SourceContext,
    db: &std::path::Path,
    mode: &str,
) -> (TemporaryDirectory, DockerActBackend) {
    let dir = TemporaryDirectory::new().unwrap();
    let source = &context.recovery.source_volume;
    let labels = bosn_core::ResourceLabels::new(
        OWNER,
        bosn_core::ResourceKind::Volume,
        "act-storage",
        &context.intent.run_id,
        bosn_core::Scope::Spec,
        &context.intent.run_id,
        &context.intent.created_at.to_string(),
        Some(bosn_core::Retention::Pinned),
    )
    .unwrap()
    .to_map();
    let volume = |name: &str, labels: serde_json::Value| {
        serde_json::json!([{
            "Name":name,"Driver":"local","Scope":"local","Options":null,"Labels":labels,
            "Mountpoint":format!("/var/lib/docker/volumes/{name}/_data")
        }])
    };
    let files = [
        (
            "source.json",
            volume(source, serde_json::to_value(labels).unwrap()),
        ),
        (
            "cache.json",
            volume(
                &context.recovery.cache_volume,
                serde_json::to_value(crate::act_engine::cache_volume_labels(OWNER, 1.0).unwrap())
                    .unwrap(),
            ),
        ),
        ("context.json", document(context)),
        (
            "input.json",
            serde_json::json!({"mode":mode,"db":db,"context":context.0,
            "context_name":context.name(),"source":source,"cache":context.recovery.cache_volume,
            "engine":context.recovery.engine_id,"engine_name":context.intent.engine_name(),
            "pin":RecoveryPin::from_intent(&context.recovery).unwrap(),
            "live":context.recovery.is_live(crate::ci::lifecycle::now_seconds())}),
        ),
    ];
    for (name, value) in files {
        std::fs::write(dir.path().join(name), value.to_string()).unwrap();
    }
    let script = dir.path().join("docker.py");
    std::fs::write(&script, DOCKER).unwrap();
    let backend = DockerActBackend::new(bosn_engine::DockerEngine::synthetic_for_test(
        "python3",
        [script.into_os_string(), dir.path().as_os_str().to_owned()],
    ));
    (dir, backend)
}

/// One case of [`replacement_registry_finalization_preserves_sources_on_every_unproven_boundary`]'s table; each runs as its own test so nextest
/// runs them in parallel (#722: run in a loop they took up to 68 s).
fn replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
    mode: &'static str,
) {
    with_registry(|source_actor, state| async move {
        let record=crate::ci::engine::cache_usage::helper::recovery_cleanup_tests::seed_source_for_lifetime(
            &source_actor,!matches!(mode,"live"|"source-path-changed"),&"c".repeat(64),"bosn-ci-cache-test",600).await;
        let context = SourceContext::from_record(&record).unwrap();
        source_actor.stop().await;
        std::fs::rename(
            state.join("registry.sqlite3"),
            state.join("unavailable.sqlite3"),
        )
        .unwrap();
        let db = state.join("replacement.sqlite3");
        drop(Registry::create_writer(&db, REPLACEMENT).unwrap());
        let daemon = Daemon::open(&db);
        let (dir, backend) = boundary(&context, &db, mode);
        let result = backend
            .dispose_retained_source(&daemon.registry, REPLACEMENT, CONTEXT, &context)
            .await;
        daemon.stop().await;
        let success = matches!(mode, "ok" | "live");
        assert_eq!(result.is_ok(), success, "{mode}: {result:?}");
        assert_eq!(dir.path().join("source.json").exists(), !success, "{mode}");
        assert_eq!(dir.path().join("context.json").exists(), !success, "{mode}");
        let reader = Registry::open_read_only(&db).unwrap();
        assert!(
            reader.act_engine(&record.intent.run_id).unwrap().is_none(),
            "invented replacement engine row"
        );
        if success {
            assert!(dir.path().join("removed").exists());
            let page = reader.events(0, 64).unwrap();
            let last = page
                .items
                .iter()
                .rev()
                .find(|e| e.kind.starts_with("ci.cache-helper.v1:"))
                .unwrap();
            let helper: bosn_registry::cache_helper::CacheHelperRecord =
                serde_json::from_str(&last.detail).unwrap();
            assert_eq!(helper.state, CacheHelperState::Removed);
        }
    });
}

mod replacement_registry_finalization_preserves_sources_on_every_unproven_boundary {
    #[test]
    fn ok() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary("ok");
    }
    #[test]
    fn live() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "live",
        );
    }
    #[test]
    fn partial() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "partial",
        );
    }
    #[test]
    fn not_absent() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "not-absent",
        );
    }
    #[test]
    fn not_disposed() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "not-disposed",
        );
    }
    #[test]
    fn bad_pin() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "bad-pin",
        );
    }
    #[test]
    fn bad_publication() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "bad-publication",
        );
    }
    #[test]
    fn wrong_root() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "wrong-root",
        );
    }
    #[test]
    fn wrong_schema() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "wrong-schema",
        );
    }
    #[test]
    fn unknown() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "unknown",
        );
    }
    #[test]
    fn report_error() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "report-error",
        );
    }
    #[test]
    fn trailing() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "trailing",
        );
    }
    #[test]
    fn oversized() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "oversized",
        );
    }
    #[test]
    fn failed_start() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "failed-start",
        );
    }
    #[test]
    fn wrong_id() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "wrong-id",
        );
    }
    #[test]
    fn lost_create() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "lost-create",
        );
    }
    #[test]
    fn foreign_command() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "foreign-command",
        );
    }
    #[test]
    fn cleanup_denied() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "cleanup-denied",
        );
    }
    #[test]
    fn lost_remove() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "lost-remove",
        );
    }
    #[test]
    fn helper_ambiguous() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "helper-ambiguous",
        );
    }
    #[test]
    fn cache_changed() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "cache-changed",
        );
    }
    #[test]
    fn source_changed() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "source-changed",
        );
    }
    #[test]
    fn source_path_changed() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "source-path-changed",
        );
    }
    #[test]
    fn engine_present() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "engine-present",
        );
    }
    #[test]
    fn engine_ambiguous() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "engine-ambiguous",
        );
    }
    #[test]
    fn context_changed() {
        super::replacement_registry_finalization_preserves_sources_on_every_unproven_boundary(
            "context-changed",
        );
    }
}

#[test]
fn lost_foreign_source_removal_reply_repeats_producer_finalization() {
    with_registry(|source_actor, state| async move {
        let record=crate::ci::engine::cache_usage::helper::recovery_cleanup_tests::seed_source_for_lifetime(
            &source_actor,true,&"c".repeat(64),"bosn-ci-cache-test",600).await;
        let context = SourceContext::from_record(&record).unwrap();
        source_actor.stop().await;
        let db = state.join("replacement.sqlite3");
        drop(Registry::create_writer(&db, REPLACEMENT).unwrap());
        let daemon = Daemon::open(&db);
        let (dir, backend) = boundary(&context, &db, "lost-volume-remove");
        assert!(
            backend
                .dispose_retained_source(&daemon.registry, REPLACEMENT, CONTEXT, &context)
                .await
                .is_err()
        );
        assert!(!dir.path().join("source.json").exists());
        assert!(dir.path().join("context.json").exists());
        backend
            .dispose_retained_source(&daemon.registry, REPLACEMENT, CONTEXT, &context)
            .await
            .unwrap();
        let calls: Vec<Vec<String>> = std::fs::read_to_string(dir.path().join("commands"))
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(calls.iter().filter(|a| a[0] == "start").count(), 2);
        assert!(!dir.path().join("context.json").exists());
        daemon.stop().await;
    });
}

const DOCKER: &str = r#"
import sys,json,sqlite3,datetime
from pathlib import Path
root=Path(sys.argv[1]);args=sys.argv[2:];data=json.loads((root/'input.json').read_text());mode=data['mode']
with (root/'commands').open('a') as out:out.write(json.dumps(args)+'\n')
context='e'*64;helperid='1'*64

def absent(kind='container'):
 print('Error: No such '+kind+': '+args[-1],file=sys.stderr);sys.exit(1)
def flag(key):return args[args.index(key)+1]
def helper():
 with sqlite3.connect(data['db']) as db:
  assert not list(db.execute("SELECT id FROM events WHERE kind LIKE 'act.engine.v1:%'"))
  h=json.loads(db.execute("SELECT detail FROM events WHERE kind LIKE 'ci.cache-helper.v1:%' ORDER BY id DESC LIMIT 1").fetchone()[0])
  assert h['intent']['role']['tool_source_finalization_v1']['context']==data['context']
  return h
if args[:2]==['image','inspect']:
 print('amd64' if '{{.Architecture}}' in args else 'sha256:'+'a'*64)
elif args[:2]==['volume','inspect']:
 name='source.json' if args[-1]==data['source'] else 'cache.json'
 if not (root/name).exists():absent('volume')
 row=json.loads((root/name).read_text())
 if (root/'created').exists():
  if mode=='cache-changed' and name=='cache.json':row[0]['Labels']={}
  if mode=='source-changed' and name=='source.json':row[0]['Labels']={}
  if mode=='source-path-changed' and name=='source.json':row[0]['Mountpoint']='/different/path'
 print(json.dumps(row))
elif args[:2]==['container','inspect']:
 if args[-1] in [context,data['context_name']]:
  if not (root/'context.json').exists():absent()
  row=json.loads((root/'context.json').read_text())
  if mode=='context-changed' and (root/'created').exists():row['Config']['Labels']={}
  print(json.dumps(row) if '--format' in args else json.dumps([row]))
 elif args[-1] in [data['engine'],data['engine_name']]:
  if mode=='engine-present':print('[{}]')
  elif mode=='engine-ambiguous':print('permission denied',file=sys.stderr);sys.exit(1)
  else:absent()
 elif (root/'removed').exists() or not (root/'created').exists():absent()
 elif mode=='helper-ambiguous' and (root/'started').exists():print('permission denied',file=sys.stderr);sys.exit(1)
 else:print((root/'created').read_text())
elif args[0]=='create':
 h=helper();assert h['state']=='pending' and h.get('container_id') is None
 assert '--cpu-shares=256' in args and '--init=false' in args
 assert ' cache tool-recovery finalize-source ' in args[-1] and '--source-quiescent --apply' in args[-1]
 assert '--source-root' in args[-1] if data['live'] else '--source-root' not in args[-1]
 assert ('--privileged' in args)==data['live']
 if not data['live']:assert flag('--cap-drop')=='ALL'
 labels=dict(args[i+1].split('=',1) for i,a in enumerate(args) if a=='--label');mounts=[]
 for i,a in enumerate(args):
  if a=='--mount':
   v=dict((x.split('=',1)+[''])[:2] for x in args[i+1].split(','));m=dict(Type=v['type'],Destination=v['target'],RW=True)
   if v['type']=='volume':m.update(Name=v['source'],Driver='local')
   else:m.update(Source=v['source'])
   mounts.append(m)
 row=dict(Id=helperid,Name='/'+flag('--name'),Config=dict(Image=args[-3],Labels=labels,Entrypoint=['/bin/sh'],Cmd=args[-2:]),
  HostConfig=dict(ReadonlyRootfs=True,Privileged=data['live'],CapDrop=None if data['live'] else ['ALL'],NetworkMode='none',Memory=536870912,NanoCpus=2000000000,
  PidsLimit=128,CpuShares=256,Tmpfs={'/tmp':'exec,size=64m'} if data['live'] else {'/tmp':'exec,size=64m','/var/lib/docker':'ro,noexec,size=1m'},Init=False,PidMode='',AutoRemove=True,RestartPolicy=dict(Name='no',MaximumRetryCount=0)),Mounts=mounts)
 if mode=='foreign-command':row['Config']['Cmd']=['-ec','sleep 600']
 if (root/'removed').exists():(root/'removed').rename(root/'previous-removed')
 (root/'created').write_text(json.dumps([row]))
 if mode=='lost-create':sys.exit(1)
 print('9'*64 if mode=='wrong-id' else helperid)
elif args[0]=='start':
 assert args==['start','--attach',helperid] and helper()['container_id']==helperid
 (root/'started').write_text('yes');(root/'fenced').write_text('yes')
 report=dict(schema_version=1,root='/bosn/cache/toolstore-v1',pin=data['pin'],disposed=mode!='not-disposed',reference_absent=mode!='not-absent',partial=mode=='partial')
 if data['live'] or mode=='bad-publication':report['publication']=dict(pin=data['pin'],generation='bad' if mode=='bad-publication' else 'd'*64,completed_at=datetime.datetime.now(datetime.timezone.utc).isoformat().replace('+00:00','Z'))
 if mode=='bad-pin':report['pin']['owner']='f'*64
 if mode=='wrong-root':report['root']='/foreign'
 if mode=='wrong-schema':report['schema_version']=2
 if mode=='unknown':report['unknown']=True
 if mode=='report-error':report['error']='sync failed'
 if mode=='trailing':print('diagnostic')
 if mode=='oversized':print(' '*4097,end='')
 print(json.dumps(report))
 if mode=='failed-start':sys.exit(1)
elif args[0]=='rm':
 assert args==['rm','-f','-v',helperid]
 if mode=='cleanup-denied':print('permission denied',file=sys.stderr);sys.exit(1)
 (root/'removed').write_text('yes')
 if mode=='lost-remove':sys.exit(1)
elif args[:2]==['volume','rm']:
 assert args==['volume','rm',data['source']] and (root/'removed').exists() and (root/'fenced').exists()
 (root/'source.json').rename(root/'source-removed.json')
 if mode=='lost-volume-remove':sys.exit(1)
elif args[:2]==['container','rm']:
 assert args==['container','rm','--volumes',context] and not (root/'source.json').exists()
 assert (root/'removed').exists() and (root/'fenced').exists()
 (root/'context.json').rename(root/'context-removed.json')
else:raise AssertionError(args)
"#;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveSource {
    cache: String,
    generation: String,
}

#[test]
#[ignore = "requires explicitly provisioned private overlay and verified offline act archive"]
fn actual_docker_replacement_registry_finalizes_and_removes_source() {
    let path = std::env::var("BOSN_ORPHAN_SOURCE_FIXTURE").unwrap();
    let fixture: LiveSource = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(fixture.cache.starts_with("bosn693-live-publication-"));
    with_registry(|source_actor, state| async move {
        let record=crate::ci::engine::cache_usage::helper::recovery_cleanup_tests::seed_source_for_lifetime(
            &source_actor,false,&fixture.generation,&fixture.cache,3600).await;
        let context = SourceContext::from_record(&record).unwrap();
        let backend = DockerActBackend::default();
        let reservation =
            super::super::helper::tests::reserve_live_reference(&backend, &record).await;
        backend.ensure_native_source_context(&record).await.unwrap();
        let (id, observed) = backend
            .observed_source_context(&context.name())
            .await
            .unwrap();
        assert_eq!(context, observed);
        let result_path = format!("{path}.result.json");
        std::fs::write(
            &result_path,
            serde_json::json!({"passed":false,"context_id":id,
            "context":context,"fixture_reservation":reservation})
            .to_string(),
        )
        .unwrap();
        source_actor.stop().await;
        std::fs::rename(
            state.join("registry.sqlite3"),
            state.join("unavailable.sqlite3"),
        )
        .unwrap();
        assert!(!state.join("registry.sqlite3").exists());
        let db = state.join("replacement.sqlite3");
        drop(Registry::create_writer(&db, REPLACEMENT).unwrap());
        let daemon = Daemon::open(&db);
        let mut cursor = super::discovery::SourceCursor::for_context(&context.recovery.owner);
        let outcome = backend
            .retry_retained_sources(&daemon.registry, REPLACEMENT, &mut cursor)
            .await;
        let advanced = outcome.as_ref().is_ok_and(|tick| {
            tick.advanced.as_deref() == Some(id.as_str()) && tick.deferred.is_empty()
        });
        daemon.stop().await;
        let reader = Registry::open_read_only(&db).unwrap();
        assert!(reader.act_engine(&record.intent.run_id).unwrap().is_none());
        let events = reader.events(0, 64).unwrap();
        let last = events
            .items
            .iter()
            .rev()
            .find(|e| e.kind.starts_with("ci.cache-helper.v1:"))
            .unwrap();
        let helper: bosn_registry::cache_helper::CacheHelperRecord =
            serde_json::from_str(&last.detail).unwrap();
        std::fs::write(&result_path,serde_json::json!({"passed":advanced,"production_worker_pass":true,"error":outcome.as_ref().err(),
            "context_id":id,"context":context,"fixture_reservation":reservation,"helper":helper,
            "pin":RecoveryPin::from_intent(&context.recovery).unwrap(),"original_registry_unavailable":true,
            "replacement_registry_has_no_engine":true}).to_string()).unwrap();
        assert!(
            advanced,
            "production worker did not finalize its discovered source: {outcome:?}"
        );
        assert_eq!(helper.state, CacheHelperState::Removed);
        backend.confirm_measurement_absent(&id).await.unwrap();
        assert!(
            !backend
                .volume_exists(&context.recovery.source_volume)
                .await
                .unwrap()
        );
    });
}
