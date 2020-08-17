#![feature(try_blocks)]

extern crate anyhow;
extern crate neovim_lib;
extern crate serde_json;
extern crate structopt;

use anyhow::{anyhow, Result};
use filter::{lookup, Line, Match, Matcher};
use neovim_lib::{Neovim, RequestHandler, Session, Value};
use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use structopt::StructOpt;

fn to_value(m: Match) -> Value {
    Value::Map(vec![(Value::from("index"), Value::from(m.index))])
}

struct EventHandler {
    matcher: Matcher,
}

impl EventHandler {
    fn new() -> Result<Self> {
        Ok(EventHandler {
            matcher: Matcher::new()?,
        })
    }
}

impl RequestHandler for EventHandler {
    fn handle_request(
        &mut self,
        name: &str,
        args: Vec<Value>,
    ) -> std::result::Result<Value, Value> {
        let result: Result<Value> = try {
            match name {
                "match" => {
                    let query = lookup(&args[0], "query")?
                        .as_str()
                        .ok_or(anyhow!("query argument is not a string"))?;
                    let context = lookup(&args[0], "context")?
                        .as_str()
                        .ok_or(anyhow!("context argument is not a string"))?;
                    let num_matches = lookup(&args[0], "num_matches")?
                        .as_u64()
                        .ok_or(anyhow!("num_matches argument is not an integer"))?;
                    let lines = itertools::process_results(
                        lookup(&args[0], "lines")?
                            .as_array()
                            .ok_or(anyhow!(
                                "lines argument {} is not an array",
                                lookup(&args[0], "lines").unwrap()
                            ))?
                            .iter()
                            .map(JSONLine::from_value),
                        |iter| iter.collect::<Vec<_>>(),
                    )?;
                    let matches = self
                        .matcher
                        .best_matches(query, context, num_matches, &lines)?;
                    Value::from(
                        matches
                            .into_iter()
                            .map(|m| to_value(m))
                            .collect::<Vec<Value>>(),
                    )
                }
                "selected" => {
                    let selected = JSONLine::from_value(&args[0])?;
                    self.matcher.update(&selected.path)?;
                    Value::from(true)
                }
                f => Err(anyhow!("No such function {}.", f))?,
            }
        };
        result.map_err(|err| Value::from(format!("{:?}", err)))
    }
}

#[derive(Debug, StructOpt)]
#[structopt(name = "sylph", about = "Fuzzy finder for use with neovim")]
struct Opts {
    #[structopt(long = "test-file", parse(from_os_str))]
    test_file: Option<PathBuf>,
}

#[derive(Deserialize)]
struct JSONLine {
    #[serde(alias = "name")]
    line: String,
    path: String,
}

impl JSONLine {
    pub fn from_value(val: &Value) -> Result<Self> {
        let path = lookup(val, "path")?
            .as_str()
            .ok_or(anyhow!("Key path is not a string."))?;
        let name = lookup(val, "name")?
            .as_str()
            .ok_or(anyhow!("Key name is not a string."))?;
        Ok(JSONLine {
            path: path.to_string(),
            line: name.to_string(),
        })
    }
}

impl Line for JSONLine {
    fn path(&self) -> &str {
        self.path.as_str()
    }

    fn line(&self) -> &str {
        self.line.as_str()
    }
}

#[derive(Deserialize)]
struct Query {
    query: String,
    launched_from: String,
    lines: Vec<JSONLine>,
    selected: JSONLine,
}

fn main() -> Result<()> {
    let opt = Opts::from_args();
    match opt.test_file {
        Some(path) => {
            let file = File::open(path).unwrap();
            let reader = BufReader::new(file);

            let mut total_score = 0.;
            let mut count = 0;
            let mut total_time = Duration::from_secs(0);
            for line in reader.lines() {
                let l = line.unwrap();
                let sl = if &l[l.len() - 1..] == "\n" {
                    &l[..l.len() - 1]
                } else {
                    &l[..]
                };
                match serde_json::from_str::<Query>(sl) {
                    Err(err) => eprintln!("{:?}", err),
                    Ok(json) => {
                        let start = Instant::now();
                        let matches = Matcher::new()?
                            .best_matches(&json.query, &json.launched_from, 10, &json.lines)
                            .unwrap();
                        let elapsed = Instant::now() - start;
                        let match_position = matches
                            .iter()
                            .position(|m| json.lines[m.index].line == json.selected.line);
                        total_score +=
                            match_position.map_or(0., |x| 0.5 * (x as f64 * -0.2).exp() + 0.5);
                        count += 1;
                        total_time += elapsed;

                        println!(
                            "query {} from {} ({} lines)",
                            json.query,
                            json.launched_from,
                            json.lines.len()
                        );
                        println!(
                            "  {:>9} {:>9} {:>9} {:>9}",
                            "total", "context", "query", "frequency"
                        );
                        for m in matches {
                            println!(
                                "  {:>9.3} {:>9.3} {:>9.3} {:>9.3} {}",
                                m.score,
                                m.context_score,
                                m.query_score,
                                m.frequency_score,
                                json.lines[m.index].path
                            );
                        }
                        println!(
                            "  correct match {} at {} in {:?} ({:?} per line)",
                            json.selected.line,
                            match_position.map_or(-1, |x| x as isize),
                            elapsed,
                            elapsed.div_f64(json.lines.len() as f64),
                        );
                    }
                }
            }

            println!("\ntotal score: {:.3}/{}", total_score, count);
            println!("total time: {:?}", total_time);
        }
        None => {
            let session = Session::new_parent().unwrap();
            let mut nvim = Neovim::new(session);

            let handler = EventHandler::new()?;
            let receiver = nvim.session.start_event_loop_channel_handler(handler);
            for _ in receiver {}
        }
    };
    Ok(())
}
