use filter::{Line, Matcher, Progress};
use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader};
use criterion::{BenchmarkId, criterion_group, criterion_main, Criterion};

#[derive(Deserialize)]
struct Location {
    path: String,
}

#[derive(Deserialize)]
struct JSONLine {
    line: String,
    location: Location,
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

fn incremental(batch_size: usize, num_results: u64, items: &Vec<Query>) {
    let matcher = Matcher::new().unwrap();
    for json in items {
        let mut inc_matcher =
            matcher.incremental_match(&json.query, &json.launched_from, num_results, &json.lines);
        let mut progress = inc_matcher.process(batch_size).unwrap();
        while progress == Progress::Working {
            progress = inc_matcher.process(batch_size).unwrap();
        }
    }
}

fn incremental_bench(c: &mut Criterion) {
    let file = File::open("tests/sylph.log").unwrap();
    let reader = BufReader::new(file);
    let mut items = Vec::new();
    let mut total = 0;
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
                total += json.lines.len();
                items.push(json);
            }
        }
    }
    c.bench_with_input(BenchmarkId::new("incremental", format!("batch 100 results 10 lines {}", total)), &items, |b, itms| {
        b.iter(|| incremental(100, 5, itms));
    });
}

criterion_group!(benches, incremental_bench);
criterion_main!(benches);
