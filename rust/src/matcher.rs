use anyhow::{anyhow, Context, Result};
use binary_heap_plus::*;
use fuzzy_matcher::skim::{SkimMatcherV2, SkimScoreConfig};
use fuzzy_matcher::FuzzyMatcher;
use itertools::process_results;
use neovim_lib::Value;
use rusqlite::{Connection, OptionalExtension};
use std::path::PathBuf;

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

    pub fn update(&mut self, entry: &str) -> Result<()> {
        self.frequency.update(entry)
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
                .map(|(i, line)| -> Result<Option<Match>> {
                    let frequency_score = self.frequency.score(line.path())?;
                    // Context score decays as the user input gets longer. We want good matches with no
                    // input, it matters less when the user has been explicit about what they want.
                    let context_score = (query.len() as f64 * -0.5).exp()
                        * if context.len() > 0 {
                            // FIXME will never give a match as context will not be a substring of
                            // line
                            self.skim_matcher
                                .fuzzy_match(&line.line(), context)
                                .unwrap_or(0) as f64
                                / line.line().len() as f64
                        } else {
                            0.
                        };
                    match self.skim_matcher.fuzzy_match(&line.line(), query) {
                        Some(query_match) => {
                            let query_score = if query.len() > 0 {
                                query_match as f64 / line.line().len() as f64
                            } else {
                                0.
                            };
                            Ok(Some(Match {
                                index: i,
                                score: frequency_score + context_score + query_score,
                                context_score,
                                frequency_score,
                                query_score,
                            }))
                        }
                        // If there is no fuzzy match, we do not include this line in the results
                        None => Ok(None),
                    }
                }),
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

/// FrequencyCounter measures freceny---a combination of frequency and recency.
/// See https://github.com/mozilla/application-services/issues/610
/// ln(e^(ln(e^t1) + t2)) + t3
struct FrequencyCounter {
    db: Connection,
}

impl FrequencyCounter {
    pub fn new() -> Result<Self> {
        let db_path = std::env::var("XDG_CACHE_DIR")
            .map(|path| PathBuf::from(path))
            .or(std::env::var("HOME").map(|path| PathBuf::from(path).join(".cache")))?
            .join("sylph/frequency.sqlite");
        let db = Connection::open(db_path)?;
        db.execute_batch("
            CREATE TABLE IF NOT EXISTS clock ( id INTEGER PRIMARY KEY CHECK (id = 0), clock INTEGER NOT NULL);
            INSERT INTO clock (id, clock) SELECT 0, 0 WHERE NOT EXISTS(SELECT 1 FROM clock);
            CREATE TABLE IF NOT EXISTS counts ( name TEXT PRIMARY KEY NOT NULL, count REAL NOT NULL);
        ")?;
        Ok(FrequencyCounter { db })
    }

    pub fn update(&mut self, entry: &str) -> Result<()> {
        let transaction = self.db.transaction()?;
        let clock = transaction
            .prepare("SELECT clock FROM clock")?
            .query_row(rusqlite::NO_PARAMS, |row| row.get::<_, isize>(0))?
            + 1;
        let count = transaction
            .prepare("SELECT count FROM counts WHERE name = ?")?
            .query_row(params![entry], |row| row.get::<_, f64>(0))
            .optional()?
            .unwrap_or(0.);
        transaction.execute(
            "INSERT OR REPLACE INTO counts (name, count) VALUES (?1, ?2)",
            params![entry, clock as f64 + (((clock as f64) - count).exp() + 1.).ln()],
        )?;
        transaction.execute("UPDATE clock SET clock = ?", params![clock])?;
        transaction.commit().context("Could not commit transaction")
    }

    pub fn scores(&self, entry: &[str]) -> Result<f64> {
        let c = self
            .db
            .prepare("SELECT clock FROM clock")?
            .query_row(rusqlite::NO_PARAMS, |row| row.get::<_, isize>(0))? as f64;
        let count = self
            .db
            .prepare("SELECT count FROM counts WHERE name = ?")?
            .query_row(params![entry], |row| row.get::<_, f64>(0))
            .optional()?
            .unwrap_or(c);
        Ok((c - count).exp())
        // Ok((-c).ln() * count /
        //     // This is the maximum possible score: e^-x * (e^1 + e^2 + e^3 + ...)
        //     ((-c).ln() * ((c + 1.).ln() - 1.) / (std::f64::consts::E - 1.)))
    }
}
