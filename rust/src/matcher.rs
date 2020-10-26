use anyhow::{anyhow, Result};
use binary_heap_plus::*;
use fuzzy_matcher::skim::{SkimMatcherV2, SkimScoreConfig};
use fuzzy_matcher::FuzzyMatcher;
use itertools::process_results;
use neovim_lib::Value;
use lru::LruCache;
use strsim::normalized_levenshtein;

pub fn lookup<'a>(val: &'a Value, key: &str) -> Result<&'a Value> {
    let map: &Vec<(Value, Value)> =
        val.as_map()
            .ok_or(anyhow!("{} is not a map. Cannot lookup key {}.", val, key))?;
    map.iter()
        .find(|x| x.0.as_str().map_or(false, |y| y == key))
        .ok_or(anyhow!(
            "Key {} not found in map. Possible keys: {}",
            key,
            map.iter()
                .map(|x| format!("{}", x.0))
                .collect::<Vec<_>>()
                .join(",")
        ))
        .map(|x| &x.1)
}

pub trait Line {
    fn path(&self) -> &str;
    fn line(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq)]
pub struct OwnedLine {
    pub path: String,
    pub line: String,
}

impl Line for OwnedLine {
    fn path(&self) -> &str {
        &self.path
    }

    fn line(&self) -> &str {
        &self.line
    }
}

#[repr(C)]
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub index: usize,
    pub score: f64,
    pub context_score: f64,
    pub query_score: f64,
    pub frequency_score: f64,
}

impl Eq for Match {}

impl PartialOrd for Match {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Match {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.score
            .partial_cmp(&other.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

pub struct Matcher {
    frequency: FrequencyCounter,
    skim_matcher: SkimMatcherV2,
}

impl Matcher {
    pub fn new() -> Result<Self> {
        Ok(Matcher {
            frequency: FrequencyCounter::new()?,
            skim_matcher: SkimMatcherV2::default()
                .use_cache(true)
                .smart_case()
                .score_config(SkimScoreConfig {
                    gap_start: -8,
                    gap_extension: -3,
                    penalty_case_mismatch: 0,
                    ..SkimScoreConfig::default()
                }),
        })
    }

    pub fn update(&mut self, entry: &str) {
        self.frequency.update(entry)
    }

    pub fn score(&self, query: &str, context: &str, index: usize, line: &str, path: &str) -> Option<Match> {
        let frequency_score = self.frequency.score(path) * 10.;
        // Context score decays as the user input gets longer. We want good matches with no
        // input, it matters less when the user has been explicit about what they want.
        let context_score = (query.len() as f64 * -0.5).exp()
            * if context.len() > 0 {
                normalized_levenshtein(line, context) * 10.
            } else {
                0.
            };
        let query_score = if query.len() > 0 {
            let whole_score = self.skim_matcher.fuzzy_match(line, query)?  as f64 / query.len() as f64;
            // Try and find path delimiters
            let slash = line.rfind('/');
            match slash {
                None => whole_score,
                Some(ind) => {
                    self.skim_matcher.fuzzy_match(&line[ind..], query).map_or(0., |x| x as f64 / query.len() as f64) + whole_score
                }
            }
        } else {
            0.
        };
        Some(Match {
            index: index,
            score: frequency_score + context_score + query_score,
            context_score,
            frequency_score,
            query_score,
        })
    }

    pub fn best_matches<L: Line>(
        &self,
        query: &str,
        context: &str,
        num_results: u64,
        lines: &[L],
    ) -> Result<Vec<Match>> {
        let mut mtchs = process_results(
            lines
                .into_iter()
                .enumerate()
                .map(|(i, line)| -> Result<Option<Match>> { Ok(self.score(query, context, i, line.line(), line.path())) }),
            |iter| {
                iter.filter_map(|x| x)
                    .fold(Vec::new(), |mut entries, mtch| {
                        if entries.len() < num_results as usize {
                            entries.push(mtch);
                            entries
                        } else {
                            let pos = entries.iter().position(|x| x.score < mtch.score);
                            match pos {
                                Some(idx) => entries[idx] = mtch,
                                None => (),
                            }
                            entries
                        }
                    })
            },
        )?;
        // TODO: only get top n entries
        mtchs.sort_unstable_by(|x, y| {
            x.score
                .partial_cmp(&y.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .reverse()
        });
        Ok(mtchs
            .into_iter()
            .take(num_results as usize)
            .collect::<Vec<_>>())
    }

    pub fn incremental_match<'a, 'b, 'c, L: Line>(
        &'b self,
        query: &'c str,
        context: &'c str,
        num_results: u64,
        lines: &'a [L],
    ) -> IncrementalMatcher<'a, 'b, 'c, L> {
        IncrementalMatcher::new(self, query, context, lines, num_results as usize)
    }
}

pub struct IncrementalMatcher<'a, 'b, 'c, L: Line> {
    matcher: &'b Matcher,
    query: &'c str,
    context: &'c str,
    lines: &'a [L],
    progressed_to: usize,
    results: BinaryHeap<Match, MinComparator>,
    num_results: usize,
}

#[derive(Eq, PartialEq, Debug)]
pub enum Progress {
    Working,
    Done(Vec<Match>),
}

impl<'a, 'b, 'c, L: Line> IncrementalMatcher<'a, 'b, 'c, L> {
    fn new(
        matcher: &'b Matcher,
        query: &'c str,
        context: &'c str,
        lines: &'a [L],
        num_results: usize,
    ) -> Self {
        IncrementalMatcher {
            matcher,
            query,
            context,
            lines,
            progressed_to: 0,
            results: BinaryHeap::<Match, MinComparator>::with_capacity_min(num_results),
            num_results,
        }
    }

    pub fn process(&mut self, num_lines: usize) -> Result<Progress> {
        if self.progressed_to == self.lines.len() {
            return Ok(Progress::Done(self.results.clone().into_sorted_vec()));
        }

        let ending_progressed_to = (self.progressed_to + num_lines).min(self.lines.len());
        let new_matches = self.matcher.best_matches(
            self.query,
            self.context,
            self.num_results as u64,
            &self.lines[self.progressed_to..ending_progressed_to],
        )?;
        for mm in new_matches {
            let m = Match {
                index: mm.index + self.progressed_to,
                ..mm
            };
            // Have room for more matches
            if self.results.len() < self.num_results {
                self.results.push(m);
            } else {
                // add match if it is bigger than the smallest best one we've found so far.
                match self.results.peek() {
                    Some(smallest) if &m > smallest => {
                        self.results.pop();
                        self.results.push(m);
                    }
                    _ => (),
                }
            }
        }
        self.progressed_to = ending_progressed_to;
        if self.progressed_to == self.lines.len() {
            Ok(Progress::Done(self.results.clone().into_sorted_vec()))
        } else {
            Ok(Progress::Working)
        }
    }
}

struct FrequencyCounter {
    cache: LruCache<String, usize>,
    clock: usize,
}

impl FrequencyCounter {
    pub fn new() -> Result<Self> {
        Ok(FrequencyCounter { cache: LruCache::new(20), clock: 0 })
    }

    pub fn update(&mut self, entry: &str) {
        self.clock += 1;
        self.cache.put(entry.to_string(), self.clock);
    }

    pub fn score(&self, entry: &str) -> f64 {
        match self.cache.peek(&entry.to_string()) { // TODO: should not have to do str -> String
            Some(c) => (*c as f64 - self.clock as f64).exp(),
            None => 0.
        }
    }
}
