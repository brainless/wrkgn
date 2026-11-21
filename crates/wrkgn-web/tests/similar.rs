use wrkgn_public_snapshot::Discovery;
use wrkgn_web::similar::{MAX_SIMILAR, SimilarIndex};

fn p(id: i64, date: i64, builder: &str, cats: &[&str], auds: &[&str]) -> Discovery {
    Discovery {
        id,
        slug: format!("p{id}"),
        name: Some(format!("P{id}")),
        description: String::new(),
        problem: None,
        audience: None,
        builder_usernames: vec![builder.into()],
        categories: cats.iter().map(|s| s.to_string()).collect(),
        audience_labels: auds.iter().map(|s| s.to_string()).collect(),
        latest_shared_at: date,
        comment_count: 0,
        searchable_text: String::new(),
    }
}
/// "dev" and "ai" are the two largest; "rare" and "mid" are small.
fn base() -> Vec<Discovery> {
    let mut v = vec![];
    for i in 0..40 {
        v.push(p(100 + i, 1000 + i, &format!("d{i}"), &["dev"], &[]));
        v.push(p(200 + i, 1000 + i, &format!("a{i}"), &["ai"], &[]));
    }
    v
}
fn ids(projects: &[Discovery], idx: &SimilarIndex, of: usize) -> Vec<i64> {
    idx.get(of)
        .iter()
        .map(|&i| projects[i as usize].id)
        .collect()
}
fn at(projects: &[Discovery], id: i64) -> usize {
    projects.iter().position(|p| p.id == id).unwrap()
}

#[test]
fn single_match_on_largest_categories_is_below_minimum() {
    let mut v = base();
    v.push(p(1, 5, "x", &["dev"], &[]));
    let idx = SimilarIndex::new(&v);
    assert!(idx.get(at(&v, 1)).is_empty());
    assert!(idx.get(at(&v, 100)).is_empty());
}

#[test]
fn rare_category_match_qualifies_and_rarer_ranks_first() {
    let mut v = base();
    v.push(p(1, 5, "x", &["rare", "mid"], &[]));
    v.push(p(2, 9, "y", &["mid"], &[]));
    v.push(p(3, 1, "z", &["rare"], &[]));
    v.push(p(4, 2, "w", &["mid"], &[]));
    v.push(p(5, 2, "w2", &["mid"], &[]));
    let idx = SimilarIndex::new(&v);
    // rare (1 other) is rarer than mid (3 others), so id 3 outranks mid-only.
    let got = ids(&v, &idx, at(&v, 1));
    assert_eq!(got[0], 3, "{got:?}");
    // among equal scores: newest first, then id.
    assert_eq!(&got[1..], &[2, 4, 5]);
}

#[test]
fn excludes_self_and_same_builder_and_needs_categories() {
    let mut v = base();
    v.push(p(1, 5, "same", &["rare"], &[]));
    v.push(p(2, 6, "same", &["rare"], &[]));
    v.push(p(3, 7, "other", &["rare"], &[]));
    v.push(p(4, 8, "n", &[], &["devs"]));
    v.push(p(5, 8, "m", &[], &["devs"]));
    let idx = SimilarIndex::new(&v);
    assert_eq!(ids(&v, &idx, at(&v, 1)), vec![3]);
    assert_eq!(ids(&v, &idx, at(&v, 3)), vec![2, 1]);
    assert!(idx.get(at(&v, 4)).is_empty());
}

#[test]
fn audience_is_secondary_and_cannot_qualify_alone() {
    let mut v = base();
    v.push(p(1, 5, "a", &["rare"], &["devs"]));
    v.push(p(2, 6, "b", &["rare"], &[]));
    v.push(p(3, 1, "c", &["rare"], &["devs"]));
    v.push(p(4, 9, "d", &["other"], &["devs"]));
    let idx = SimilarIndex::new(&v);
    // Same category score; shared audience beats newer date.
    assert_eq!(ids(&v, &idx, at(&v, 1)), vec![3, 2]);
}

#[test]
fn ties_break_by_date_then_id_and_caps_at_six() {
    let mut v = base();
    for i in 0..9 {
        v.push(p(
            10 + i,
            if i < 4 { 50 } else { 60 },
            &format!("b{i}"),
            &["rare"],
            &[],
        ));
    }
    let idx = SimilarIndex::new(&v);
    let got = ids(&v, &idx, at(&v, 10));
    assert_eq!(got.len(), MAX_SIMILAR);
    assert_eq!(got, vec![14, 15, 16, 17, 18, 11]);
}

#[test]
fn deterministic_across_builds_and_input_independent_of_scratch_state() {
    let mut v = base();
    for i in 0..20 {
        let cats: &[&str] = if i % 3 == 0 {
            &["rare", "dev"]
        } else {
            &["mid", "ai"]
        };
        v.push(p(10 + i, 20 + i % 4, &format!("b{i}"), cats, &["devs"]));
    }
    let (a, b) = (SimilarIndex::new(&v), SimilarIndex::new(&v));
    for i in 0..v.len() {
        assert_eq!(a.get(i), b.get(i));
    }
}
