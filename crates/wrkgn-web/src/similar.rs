//! Precomputed "Related projects": similar projects by shared categories.
//!
//! Built once per snapshot load from the in-memory discovery records. Requests
//! only look up a short list of indices into `DiscoveryIndex::projects`.
use std::collections::HashMap;
use wrkgn_public_snapshot::Discovery;

pub const MAX_SIMILAR: usize = 6;
/// Minimum summed category rarity weight, in millionths of a nat.
pub const MIN_SCORE: u64 = 500_000;
const SCALE: f64 = 1_000_000.0;

#[derive(Debug, Default)]
pub struct SimilarIndex {
    offsets: Vec<u32>,
    data: Vec<u32>,
}
impl SimilarIndex {
    /// Indices into the discovery project list for the project at `index`, best first.
    pub fn get(&self, index: usize) -> &[u32] {
        match (self.offsets.get(index), self.offsets.get(index + 1)) {
            (Some(&a), Some(&b)) => &self.data[a as usize..b as usize],
            _ => &[],
        }
    }
    pub fn new(projects: &[Discovery]) -> Self {
        let n = projects.len();
        let postings = |labels: &dyn Fn(&Discovery) -> &Vec<String>| {
            let mut ids: HashMap<&str, usize> = HashMap::new();
            let mut lists: Vec<Vec<u32>> = vec![];
            let mut per_project: Vec<Vec<usize>> = Vec::with_capacity(n);
            for (i, p) in projects.iter().enumerate() {
                let mut own: Vec<usize> = vec![];
                for label in labels(p) {
                    let id = *ids.entry(label.as_str()).or_insert_with(|| {
                        lists.push(vec![]);
                        lists.len() - 1
                    });
                    if !own.contains(&id) {
                        own.push(id);
                        lists[id].push(i as u32);
                    }
                }
                per_project.push(own);
            }
            let weights: Vec<u64> = lists
                .iter()
                .map(|l| ((n as f64 / l.len() as f64).ln() * SCALE).round() as u64)
                .collect();
            (lists, weights, per_project)
        };
        let (cat_lists, cat_w, cat_of) = postings(&|p| &p.categories);
        let (aud_lists, aud_w, aud_of) = postings(&|p| &p.audience_labels);
        // The two largest categories, determined from this snapshot: a match on
        // only one of them cannot meet the minimum.
        let mut by_size: Vec<usize> = (0..cat_lists.len()).collect();
        by_size.sort_by_key(|&c| std::cmp::Reverse(cat_lists[c].len()));
        let broad_max = by_size.iter().take(2).map(|&c| cat_w[c]).max().unwrap_or(0);
        let min_score = MIN_SCORE.max(broad_max + 1);

        let mut by_builder: HashMap<&str, Vec<u32>> = HashMap::new();
        for (i, p) in projects.iter().enumerate() {
            for b in &p.builder_usernames {
                by_builder.entry(b.as_str()).or_default().push(i as u32);
            }
        }

        let mut cat_score = vec![0u64; n];
        let mut aud_score = vec![0u64; n];
        let mut excluded = vec![u32::MAX; n]; // stamp: project index that excluded it
        let mut seen = vec![u32::MAX; n];
        let mut touched: Vec<u32> = vec![];
        let mut offsets = Vec::with_capacity(n + 1);
        let mut data: Vec<u32> = vec![];
        offsets.push(0);
        for i in 0..n {
            if !cat_of[i].is_empty() {
                let stamp = i as u32;
                excluded[i] = stamp;
                for b in &projects[i].builder_usernames {
                    for &o in &by_builder[b.as_str()] {
                        excluded[o as usize] = stamp;
                    }
                }
                touched.clear();
                for (pass, lists, ids, w) in [
                    (0, &cat_lists, &cat_of[i], &cat_w),
                    (1, &aud_lists, &aud_of[i], &aud_w),
                ] {
                    for &c in ids {
                        for &o in &lists[c] {
                            let o = o as usize;
                            if excluded[o] == stamp {
                                continue;
                            }
                            if seen[o] != stamp {
                                seen[o] = stamp;
                                cat_score[o] = 0;
                                aud_score[o] = 0;
                                touched.push(o as u32);
                            }
                            if pass == 0 {
                                cat_score[o] += w[c];
                            } else {
                                aud_score[o] += w[c];
                            }
                        }
                    }
                }
                // Audience-only candidates never qualify: they fail min_score below.
                let better = |a: u32, b: u32| {
                    let (a, b) = (a as usize, b as usize);
                    let key = |x: usize| {
                        (
                            std::cmp::Reverse(cat_score[x]),
                            std::cmp::Reverse(aud_score[x]),
                            std::cmp::Reverse(projects[x].latest_shared_at),
                            projects[x].id,
                        )
                    };
                    key(a) < key(b)
                };
                let mut top: Vec<u32> = Vec::with_capacity(MAX_SIMILAR + 1);
                for &o in &touched {
                    if cat_score[o as usize] < min_score {
                        continue;
                    }
                    let pos = top.partition_point(|&t| better(t, o));
                    if pos < MAX_SIMILAR {
                        top.insert(pos, o);
                        top.truncate(MAX_SIMILAR);
                    }
                }
                data.extend_from_slice(&top);
            }
            offsets.push(data.len() as u32);
        }
        data.shrink_to_fit();
        Self { offsets, data }
    }
}
