extern crate anyhow;
extern crate nvim_rs;
extern crate serde_json;
extern crate structopt;

use anyhow::{anyhow, Result};
use filter::{lookup, Line, Match, Matcher};
use nvim_rs::{compat::tokio::Compat, create::tokio as create, neovim::Neovim, Handler, Value};
use serde::Deserialize;
use std::error::Error;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use structopt::StructOpt;
use tokio::fs::File as TokioFile;

fn to_value(m: Match) -> Value {
    Value::Map(vec![(Value::from("index"), Value::from(m.index))])
}

#[derive(Clone)]
struct EventHandler {
    matcher: std::sync::Arc<std::sync::Mutex<Matcher>>,
}

impl EventHandler {
    fn new() -> Result<Self> {
        Ok(EventHandler {
            matcher: std::sync::Arc::new(Matcher::new()?.into()),
        })
    }
}

#[async_trait::async_trait]
impl Handler for EventHandler {
    type Writer = Compat<TokioFile>;
    async fn handle_request(
        &self,
        name: String,
        args: Vec<Value>,
        _neovim: Neovim<Compat<TokioFile>>,
    ) -> std::result::Result<Value, Value> {
        let result: Result<Value> = (|| -> Result<Value> {
            match name.as_str() {
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
                        .lock()
                        .map_err(|_| anyhow!("state mutex poisoned"))?
                        .best_matches(query, context, num_matches, &lines)?;
                    Ok(Value::from(
                        matches
                            .into_iter()
                            .map(|m| to_value(m))
                            .collect::<Vec<Value>>(),
                    ))
                }
                "selected" => {
                    let selected = JSONLine::from_value(&args[0])?;
                    self.matcher
                        .lock()
                        .map_err(|_| anyhow!("state mutex poisoned"))?
                        .update(&selected.location.path);
                    Ok(Value::from(true))
                }
                f => Err(anyhow!("No such function {}.", f))?,
            }
        })();
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
    location: Location,
}

#[derive(Deserialize)]
struct Location {
    path: String,
}

impl JSONLine {
    pub fn from_value(val: &Value) -> Result<Self> {
        let path = lookup(lookup(val, "location")?, "path")?
            .as_str()
            .ok_or(anyhow!("Key path is not a string."))?;
        let name = lookup(val, "name")?
            .as_str()
            .ok_or(anyhow!("Key name is not a string."))?;
        Ok(JSONLine {
            location: Location {
                path: path.to_string(),
            },
            line: name.to_string(),
        })
    }
}

impl Line for JSONLine {
    fn path(&self) -> &str {
        self.location.path.as_str()
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

#[tokio::main]
async fn main() -> Result<()> {
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
                                json.lines[m.index].location.path
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
            let handler = EventHandler::new()?;

            let (nvim, io_handler) = create::new_parent(handler).await.unwrap();

            // Any error should probably be logged, as stderr is not visible to users.
            match io_handler.await {
                Err(joinerr) => eprintln!("Error joining IO loop: '{}'", joinerr),
                Ok(Err(err)) => {
                    if !err.is_reader_error() {
                        // One last try, since there wasn't an error with writing to the
                        // stream
                        nvim.err_writeln(&format!("Error: '{}'", err))
                            .await
                            .unwrap_or_else(|e| {
                                // We could inspect this error to see what was happening, and
                                // maybe retry, but at this point it's probably best
                                // to assume the worst and print a friendly and
                                // supportive message to our users
                                eprintln!("Well, dang... '{}'", e);
                            });
                    }

                    if !err.is_channel_closed() {
                        // Closed channel usually means neovim quit itself, or this plugin was
                        // told to quit by closing the channel, so it's not always an error
                        // condition.
                        eprintln!("Error: '{}'", err);

                        let mut source = err.source();

                        while let Some(e) = source {
                            eprintln!("Caused by: '{}'", e);
                            source = e.source();
                        }
                    }
                }
                Ok(Ok(())) => {}
            }
        }
    };
    Ok(())
}
