//! Measure log streaming + graph layout on a real repository.
//!
//!     cargo run --release -p git-core --example graph_bench -- /path/to/repo [--draw 40]
//!
//! The number that matters most for the UI is "first screen": how long until
//! the first 100 rows are laid out and could be painted.

use std::mem::size_of;
use std::sync::Arc;
use std::time::Instant;

use git_core::{Git, GitChoice, LogOptions, Oid, Repo};
use git_graph::{GraphBuilder, Row, Segment};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| ".".into());
    let draw: usize = match (args.next().as_deref(), args.next()) {
        (Some("--draw"), Some(n)) => n.parse()?,
        _ => 0,
    };

    let repo = Repo::open(Arc::new(Git::resolve(GitChoice::Auto)?), &path)?;
    if !repo.has_commit_graph() {
        eprintln!("note: no commit-graph file; the first screen will be slow.");
        eprintln!(
            "      run `git commit-graph write --reachable` to fix (the app will do it itself)."
        );
    }
    let started = Instant::now();
    let mut graph = GraphBuilder::new();
    let mut first_screen = None;
    let mut max_width = 0;
    let mut segments = 0usize;
    let mut drawn = Vec::new();

    for commit in repo.log(&LogOptions::default())? {
        let commit = commit?;
        let row = graph.push(commit.id, &commit.parents);
        max_width = max_width.max(row.width);
        segments += row.above.len() + row.below.len();
        if drawn.len() < draw {
            drawn.push((row, format!("{} {}", commit.id.short(), commit.summary)));
        }
        if graph.len() == 100 {
            first_screen = Some(started.elapsed());
        }
    }
    let total = started.elapsed();

    if draw > 0 {
        let rows: Vec<_> = drawn.iter().map(|(r, _)| r.clone()).collect();
        let mut labels = drawn.iter().map(|(_, l)| l.clone());
        print!(
            "{}",
            git_graph::ascii::render(&rows, |_| labels.next().unwrap_or_default())
        );
        println!();
    }

    println!("repository   {}", repo.root().display());
    println!("commits      {}", graph.len());
    println!("first screen {:?}", first_screen.unwrap_or(total));
    println!("total        {total:?}");
    println!("max width    {max_width} lanes");
    // Row structs (with their Vec headers) plus segment payloads; ignores allocator slack.
    let bytes = graph.len() * size_of::<Row<Oid>>() + segments * size_of::<Segment>();
    println!("segments     {segments}");
    println!("row memory   ~{:.1} MiB", bytes as f64 / (1 << 20) as f64);
    Ok(())
}
