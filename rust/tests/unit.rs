use serde::Deserialize;
use sylph::{Line, Matcher, ThreadedMatcher, Progress};
use std::io::{BufRead, BufReader};
use std::fs::File;

#[derive(Deserialize)]
struct JSONLine {
    name: String,
    path: String,
}

impl Line for JSONLine {
    fn path(&self) -> &str {
        self.path.as_str()
    }
}

#[derive(Deserialize)]
struct Query {
    query: String,
    launched_from: String,
    lines: Vec<JSONLine>,
    selected: JSONLine,
}

#[test]
fn incremental_same_as_batch() {
    let file = File::open("tests/sylph.log").unwrap();
    let reader = BufReader::new(file);
    let matcher = Matcher::new().unwrap();
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
                let mtchs_batch = matcher.best_matches(&json.query, &json.launched_from, 10, &json.lines).unwrap();
                let mut inc_matcher = matcher.incremental_match(&json.query, &json.launched_from, 10, &json.lines);
                let mut progress = inc_matcher.process(10).unwrap();
                while progress == Progress::Working {
                    progress = inc_matcher.process(10).unwrap();
                }
                match progress {
                    Progress::Done(results) => assert!(mtchs_batch == results, "{:#?} {:#?}", mtchs_batch, results),
                    _ => assert!(false),
                }
            }
        }
    }
}
