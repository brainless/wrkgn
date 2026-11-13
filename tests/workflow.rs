use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use wrkgn::collector::{CollectionConfig, Collector, FetchError, HnApi};
use wrkgn::extraction::{Extraction, Mention};
use wrkgn::processing::{Model, Options};
use wrkgn::workflow::{self, ApprovalPolicy, ThreadOptions};
use wrkgn::{publication, store::Store};

struct Api;
impl HnApi for Api {
    fn fetch_item(&self, id: i64) -> Result<Option<Value>, FetchError> {
        Ok(Some(match id {
            100 => json!({"id":100,"type":"story","kids":[101]}),
            101 => json!({"id":101,"parent":100,"by":"alice","time":1000,
                "text":"Building Alpha https://alpha.test"}),
            _ => panic!("unexpected item"),
        }))
    }
}
struct Fixture(AtomicUsize);
impl Model for Fixture {
    fn name(&self) -> &str {
        "workflow-fixture"
    }
    fn extract(&self, _: &str) -> Result<Extraction, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Extraction {
            mentions: vec![Mention {
                source_item_id: 101,
                name: Some("Alpha".into()),
                urls: vec!["https://alpha.test".into()],
                description: "Alpha organizes work".into(),
                problem: None,
                audience: None,
                source_excerpt: "Building Alpha".into(),
                builders: vec![],
                categories: vec![],
                audience_labels: vec![],
            }],
        })
    }
}

#[test]
fn explicit_manual_policy_exports_without_approval_and_forced_rerun_is_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("db")).unwrap();
    let collector = Collector::new(Api, CollectionConfig::default());
    let model = Fixture(AtomicUsize::new(0));
    let options = Options::default();
    let destination = dir.path().join("export");
    for (reprocess, policy, calls, pending) in [
        (false, ApprovalPolicy::Manual, 1, 1),
        (false, ApprovalPolicy::Manual, 1, 1),
        (false, ApprovalPolicy::ApproveThread, 1, 0),
        (true, ApprovalPolicy::Manual, 2, 1),
    ] {
        let mut approved = None;
        workflow::sync_thread(
            &mut store,
            &collector,
            &model,
            100,
            &ThreadOptions {
                processing: &options,
                reprocess,
                approval: policy,
                destination: &destination,
            },
            |event| {
                if let workflow::Event::Exported {
                    approved: count, ..
                } = event
                {
                    approved = count;
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(model.0.load(Ordering::SeqCst), calls);
        assert_eq!(
            publication::pending_mentions(&mut store).unwrap().len(),
            pending
        );
        assert_eq!(
            approved,
            if policy == ApprovalPolicy::Manual {
                None
            } else {
                Some(1)
            }
        );
    }
}
