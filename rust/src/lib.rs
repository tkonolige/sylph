#![feature(try_blocks)]

extern crate anyhow;
extern crate fuzzy_matcher;
extern crate itertools;
extern crate serde;
extern crate sublime_fuzzy;

use anyhow::{anyhow, Context, Result};
use fuzzy_matcher::skim::{SkimMatcherV2, SkimScoreConfig};
use fuzzy_matcher::FuzzyMatcher;
use itertools::process_results;
use neovim_lib::Value;
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_char;

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

#[repr(C)]
#[derive(Deserialize, Debug, Clone)]
pub struct Line<'a> {
    pub path: &'a str,
    pub name: &'a str,
}

impl<'a> Line<'a> {
    pub fn from_value(val: &'a Value) -> Result<Self> {
        let path = lookup(val, "path")?
            .as_str()
            .ok_or(anyhow!("Key path is not a string."))?;
        let name = lookup(val, "name")?
            .as_str()
            .ok_or(anyhow!("Key name is not a string."))?;
        Ok(Line { path, name })
    }
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct Match {
    pub index: usize,
    pub score: f64,
    pub context_score: f64,
    pub query_score: f64,
    pub frequency_score: f64,
}

#[repr(C)]
#[derive(Debug)]
pub struct RawLine {
    name: *const c_char,
    path: *const c_char,
}

pub struct Matcher {
    frequency: FrequencyCounter,
}

#[no_mangle]
pub extern "C" fn new_matcher() -> *mut Matcher {
    Box::into_raw(Box::new(Matcher::new()))
}

#[no_mangle]
pub extern "C" fn free_matcher(matcher: *mut Matcher) {
    unsafe { Box::from_raw(matcher) };
    ()
}

#[no_mangle]
pub extern "C" fn update_matcher(matcher: *mut Matcher, path: *const c_char) {
    unsafe {
        let _ = matcher
            .as_mut()
            .ok_or(anyhow!("Invlid matcher"))
            .and_then(|m| Ok(m.update(CStr::from_ptr(path).to_str()?)));
    }
}

impl Matcher {
    pub fn new() -> Self {
        Matcher {
            frequency: FrequencyCounter::new(),
        }
    }

    pub fn update(&mut self, entry: &str) {
        self.frequency.update(entry)
    }

    pub fn best_matches(
        &self,
        query: &str,
        context: &str,
        num_matches: u64,
        lines: &[Line],
    ) -> Result<Vec<Match>> {
        let matcher = SkimMatcherV2::default()
            .use_cache(true)
            .smart_case()
            .score_config(SkimScoreConfig {
                gap_start: -8,
                gap_extension: -3,
                ..SkimScoreConfig::default()
            });
        let mut mtchs = lines
            .into_iter()
            .enumerate()
            .filter_map(|(i, line)| {
                let frequency_score = self.frequency.score(line.name);
                // Context score decays as the user input gets longer. We want good matches with no
                // input, it matters less when the user has been explicit about what they want.
                let context_score = (query.len() as f64 * -0.5).exp()
                    * if context.len() > 0 {
                        matcher.fuzzy_match(&line.name, context).unwrap_or(0) as f64
                            / line.name.len() as f64
                    } else {
                        0.
                    };
                let query_score = if query.len() > 0 {
                    // If there is no fuzzy match, we do not include this line in the results
                    matcher.fuzzy_match(&line.name, query)? as f64 / line.name.len() as f64
                } else {
                    0.
                };
                Some(Match {
                    index: i,
                    score: frequency_score + context_score + query_score,
                    context_score,
                    frequency_score,
                    query_score,
                })
            })
            .fold(Vec::new(), |mut entries, mtch| {
                if entries.len() < num_matches as usize {
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
            });
        // TODO: only get top n entries
        mtchs.sort_unstable_by(|x, y| {
            x.score
                .partial_cmp(&y.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .reverse()
        });
        Ok(mtchs
            .into_iter()
            .take(num_matches as usize)
            .collect::<Vec<_>>())
    }
}

#[no_mangle]
pub extern "C" fn best_matches_c(
    matcher: *const Matcher,
    query: *const c_char,
    context: *const c_char,
    num_matches: u64,
    lines_ptr: *const RawLine,
    num_lines: u64,
    result_ptr: *mut Match,
    num_results: *mut u64,
) -> i64 {
    let res = std::panic::catch_unwind(|| {
        let mtchs: Result<Vec<Match>> = try {
            // TODO: avoid allocating a vector?
            let lines = unsafe {
                process_results(
                    std::slice::from_raw_parts(lines_ptr, num_lines as usize)
                        .into_iter()
                        .map(|l| -> Result<Line> {
                            if l.name.is_null() {
                                Err(anyhow!("Name is null"))?
                            };
                            if l.path.is_null() {
                                Err(anyhow!("Path is null"))?
                            };
                            Ok(Line {
                                name: CStr::from_ptr(l.name)
                                    .to_str()
                                    .context("Invalid string for name")?,
                                path: CStr::from_ptr(l.path)
                                    .to_str()
                                    .context("Invalid string for path")?,
                            })
                        }),
                    |itr| itr.collect::<Vec<_>>(),
                )?
            };
            let q = unsafe { CStr::from_ptr(query).to_str()? };
            let c = unsafe { CStr::from_ptr(context).to_str()? };

            unsafe {
                matcher.as_ref().context("invalid pointer")?.best_matches(
                    q,
                    c,
                    num_matches,
                    lines.as_ref(),
                )?
            }
        };
        match mtchs {
            Ok(r) => {
                let result =
                    unsafe { std::slice::from_raw_parts_mut(result_ptr, num_matches as usize) };
                unsafe { *num_results = r.len() as u64 };
                for i in 0..r.len() {
                    result[i] = r[i].clone()
                }
                0
            }
            Err(_) => 1,
        }
    });
    match res {
        Ok(r) => r,
        Err(_) => 1,
    }
}

/// FrequencyCounter measures freceny---a combination of frequency and recency.
/// The score for an entry once is e^(t-x) where t is the time at which it was used and x is the
/// current time. Thus the total score for an entry is e^-x * (e^t1 + e^t2 + e^t3 + ...). We can
/// store e^t1 + e^t2 + e^t3 + ... as a single number. Updates can just be added to this number.
struct FrequencyCounter {
    counts: HashMap<String, f64>,
    clock: usize,
}

impl FrequencyCounter {
    pub fn new() -> Self {
        FrequencyCounter {
            counts: HashMap::new(),
            clock: 0,
        }
    }

    pub fn update(&mut self, entry: &str) {
        self.clock += 1;
        *self.counts.entry(entry.to_string()).or_insert(0.) += (self.clock as f64).exp();
    }

    pub fn score(&self, entry: &str) -> f64 {
        let c = self.clock as f64;
        (-c).exp() * *self.counts.get(entry).unwrap_or(&0.) /
            // This is the maximum possible score: e^-x * (e^1 + e^2 + e^3 + ...)
            ((-c).exp() * ((c + 1.).exp() - 1.) / (std::f64::consts::E - 1.))
    }
}
