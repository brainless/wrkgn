use serde::Deserialize;
use wrkgn_public_snapshot::Discovery;
use wrkgn_web::discovery::{DiscoveryIndex, Sort, State, read_state};

fn project(id: i64, date: i64, comments: i64, text: &str) -> Discovery {
    Discovery {
        id,
        slug: format!("project-{id}"),
        name: Some(format!("Project {id}")),
        description: text.into(),
        problem: None,
        audience: None,
        builder_usernames: vec!["samplebuilder".into()],
        categories: vec![
            if id % 2 == 0 {
                "education"
            } else {
                "productivity"
            }
            .into(),
        ],
        audience_labels: vec!["researchers".into()],
        latest_shared_at: date,
        comment_count: comments,
        searchable_text: text.into(),
    }
}

#[derive(Deserialize)]
struct Case {
    name: String,
    input: String,
    status: Option<u16>,
    location: Option<String>,
}
#[derive(Deserialize)]
struct Cases {
    project_count: usize,
    cases: Vec<Case>,
}

#[test]
fn shared_canonical_url_cases() {
    let cases: Cases = serde_json::from_str(include_str!("fixtures/canonical-urls.json")).unwrap();
    let index = DiscoveryIndex::new(
        (1..=cases.project_count as i64)
            .map(|id| project(id, id, id % 3, "field notes café 🦀"))
            .collect(),
    );
    for case in cases.cases {
        let (path, query) = match case.input.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (case.input.as_str(), None),
        };
        let result = index.resolve(path, query);
        assert_eq!(
            result.redirect.as_ref().map(|r| r.status),
            case.status,
            "{}",
            case.name
        );
        assert_eq!(
            result.redirect.as_ref().map(|r| r.location.as_str()),
            case.location.as_deref(),
            "{}",
            case.name
        );
    }
}

#[test]
fn matching_sorts_before_pagination_with_stable_ties() {
    let index = DiscoveryIndex::new(vec![
        project(3, 200, 2, "field notes"),
        project(2, 200, 2, "field notes"),
        project(1, 100, 1, "samplebuilder"),
        project(4, 50, 1, "other"),
    ]);
    let ids = |state: State| {
        index
            .page(&state)
            .indices
            .iter()
            .map(|&i| index.projects[i].id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(State::default()), [2, 3, 1, 4]);
    assert_eq!(
        ids(State {
            sort: Sort::DateAsc,
            ..State::default()
        }),
        [4, 1, 2, 3]
    );
    assert_eq!(
        ids(State {
            sort: Sort::CommentsAsc,
            ..State::default()
        }),
        [1, 4, 2, 3]
    );
    assert_eq!(
        ids(State {
            sort: Sort::CommentsDesc,
            ..State::default()
        }),
        [2, 3, 1, 4]
    );
    assert_eq!(
        ids(State {
            q: "SAMPLEBUILDER".into(),
            ..State::default()
        }),
        [1]
    );
    assert_eq!(
        ids(State {
            category: vec!["education".into(), "productivity".into()],
            ..State::default()
        }),
        [2, 3, 1, 4]
    );
    assert_eq!(
        ids(State {
            category: vec!["education".into()],
            audience: vec!["missing".into()],
            ..State::default()
        }),
        Vec::<i64>::new()
    );
    let large = DiscoveryIndex::new((1..=65).map(|id| project(id, id, 0, "field")).collect());
    let second = large.page(&State {
        page: 2,
        ..State::default()
    });
    assert_eq!(
        (second.total, second.pages, second.indices.len()),
        (65, 3, 30)
    );
    assert_eq!(large.projects[second.indices[0]].id, 35);
}

#[test]
fn unicode_and_short_substrings_match_default_case_mapping() {
    let index = DiscoveryIndex::new(vec![project(1, 1, 0, "CAFÉ ΣΟΣ İ 🦀")]);
    for q in ["c", "ca", "café", "σος", "i", "i\u{307}", "🦀"] {
        assert_eq!(
            index
                .page(&State {
                    q: q.into(),
                    ..State::default()
                })
                .total,
            1,
            "{q}"
        );
    }
    assert_eq!(
        index
            .page(&State {
                q: "cafe".into(),
                ..State::default()
            })
            .total,
        0
    );
    assert_eq!(
        read_state("/", Some("q=%EF%BB%BFfield%EF%BB%BF")).q,
        "field"
    );
    assert_eq!(
        read_state("/", Some("q=%C2%85field%C2%85")).q,
        "\u{85}field\u{85}"
    );
}

#[test]
fn empty_index_has_one_empty_page_and_redirects_later_pages() {
    let index = DiscoveryIndex::new(vec![]);
    assert_eq!(index.page(&State::default()).pages, 1);
    let result = index.resolve("/projects/page/2/", None);
    assert_eq!(result.redirect.unwrap().location, "/");
    assert!(index.options.category.is_empty());
}
