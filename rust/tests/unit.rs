use filter::{Line, Matcher, Progress};
use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader};

#[derive(Deserialize)]
struct JSONLine {
    name: String,
    path: String,
}

impl Line for JSONLine {
    fn path(&self) -> &str {
        self.path.as_str()
    }
    fn line(&self) -> &str {
        self.name.as_str()
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
    let mut matcher = Matcher::new().unwrap();
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
                let mtchs_batch = matcher
                    .best_matches(&json.query, &json.launched_from, 5, &json.lines)
                    .unwrap();
                let mut inc_matcher =
                    matcher.incremental_match(json.query, json.launched_from, 5);
                inc_matcher.feed_lines(json.lines);
                let mut progress = inc_matcher.process(10).unwrap();
                while progress == Progress::Working {
                    progress = inc_matcher.process(10).unwrap();
                }
                match progress {
                    Progress::Done(results) => {
                        println!("{:#?}", mtchs_batch);
                        println!("{:#?}", results);
                        for i in 0..mtchs_batch.len() {
                            assert!(
                                mtchs_batch[i] == results[i],
                                "{}th result:\n{:#?}\nvs\n{:#?}",
                                i,
                                mtchs_batch[i],
                                results[i]
                            )
                        }
                    }
                    _ => assert!(false),
                }
            }
        }
    }
}
