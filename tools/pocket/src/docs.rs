//! The engine's documentation searched by topic (docs/mcp.md, `docs_search`; `pocket docs`): the
//! sections of docs/*.md, docs/design/*.md and docs/generated/*.md that best match a query, each
//! under its file and heading, so an agent reads a few paragraphs rather than a 70 KB file.

use crate::manifest::Workspace;
use crate::report::Report;
use anyhow::Result;
use serde_json::json;
use std::path::Path;
use std::time::Instant;

struct Section {
    file: String,
    heading: String,
    text: String,
    lower: String,
}

const STOP: &[&str] = &["the", "a", "an", "of", "to", "in", "and", "or", "how", "what", "is", "for", "on", "with", "do", "i", "it", "its", "by", "be", "can", "from", "at", "as", "that", "this"];

// The query's words, lowercased and cut to a stem (an ending of -ing, -ed, -es, -s or -e dropped:
// they match as substrings, so "saving" and "save" both find "saves"), and a dotted name
// ("Animator.locomotion") both whole and in its parts.
fn terms(query: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for word in query.to_lowercase().split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')) {
        let word = word.trim_matches('.');
        let mut parts = vec![word.to_string()];
        if word.contains('.') {
            parts.extend(word.split('.').map(String::from));
        }
        for p in parts {
            let mut p = p;
            for end in ["ing", "ed", "es", "s", "e"] {
                if p.len() > end.len() + 2 && p.ends_with(end) && !p.ends_with("ss") {
                    p.truncate(p.len() - end.len());
                    break;
                }
            }
            if p.len() >= 2 && !STOP.contains(&p.as_str()) && !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

fn sections(docs: &Path) -> Vec<Section> {
    let mut out = vec![];
    for dir in [docs.to_path_buf(), docs.join("design"), docs.join("generated")] {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        // The engine's own record (what is built, the benchmark, the plans) is not how to make a game.
        const SKIP: &[&str] = &["status.md", "roadmap.md", "agent-eval.md", "positioning.md", "ue5-lessons.md", "unity-lessons.md", "build-system.md"];
        let mut files: Vec<_> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "md") && !p.file_name().and_then(|n| n.to_str()).is_some_and(|n| SKIP.contains(&n)))
            .collect();
        files.sort();
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let file = path.strip_prefix(docs).unwrap_or(&path).display().to_string();
            let mut heading = String::new();
            let mut body = String::new();
            let flush = |heading: &str, body: &mut String, out: &mut Vec<Section>| {
                if !body.trim().is_empty() {
                    let text = body.trim().to_string();
                    out.push(Section { file: file.clone(), heading: heading.to_string(), lower: format!("{} {}", heading, text).to_lowercase(), text });
                }
                body.clear();
            };
            for line in text.lines() {
                if let Some(h) = line.strip_prefix("## ").or_else(|| line.strip_prefix("### ")).or_else(|| line.strip_prefix("# ")) {
                    flush(&heading, &mut body, &mut out);
                    heading = h.trim().to_string();
                } else {
                    body.push_str(line);
                    body.push('\n');
                }
            }
            flush(&heading, &mut body, &mut out);
        }
    }
    out
}

/// The sections that best match `query`, best first: each term's count weighed by how rare it is
/// across the sections, a term in the heading worth four, the whole query as a phrase worth one of
/// each, and long sections not favoured for their length alone.
pub fn search(ws: &Workspace, query: &str, limit: usize) -> Result<Report> {
    let t0 = Instant::now();
    let docs = ws.root.join("docs");
    let all = sections(&docs);
    let ts = terms(query);
    let mut rep = Report::success("docs", "");
    if ts.is_empty() {
        rep.ok = false;
        rep.summary = "give a topic to search for (\"render scale\", \"tilemap sight\", \"Animator.locomotion\")".into();
        return Ok(rep);
    }
    let n = all.len().max(1) as f64;
    let idf: Vec<f64> = ts.iter().map(|t| (n / (1.0 + all.iter().filter(|s| s.lower.contains(t.as_str())).count() as f64)).ln().max(0.1)).collect();
    let phrase = query.trim().to_lowercase();
    let mut scored: Vec<(f64, usize)> = all
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let words = (s.lower.len() as f64 / 6.0).max(50.0);
            let heading = s.heading.to_lowercase();
            let mut score = 0.0;
            let mut matched = 0;
            for (t, w) in ts.iter().zip(&idf) {
                let count = s.lower.matches(t.as_str()).count() as f64;
                if count > 0.0 {
                    matched += 1;
                }
                score += w * (count / (count + 1.5 * words / 300.0)) + if heading.contains(t.as_str()) { 4.0 * w } else { 0.0 };
            }
            if ts.len() > 1 && s.lower.contains(&phrase) {
                score += idf.iter().sum::<f64>();
            }
            score *= matched as f64 / ts.len() as f64;   // sections with every term first
            (score, i)
        })
        .filter(|(s, _)| *s > 0.0)
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let results: Vec<_> = scored
        .iter()
        .take(limit.clamp(1, 10))
        .map(|(score, i)| {
            let s = &all[*i];
            // At most about 2,500 characters, cut at a paragraph's end.
            let mut text = s.text.clone();
            if text.len() > 2500 {
                let mut cut = text[..2500].rfind("\n\n").unwrap_or(2500);
                while !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                text.truncate(cut);
                text.push_str(&format!("\n[... the section goes on: read {} under '{}']", s.file, s.heading));
            }
            json!({ "file": format!("docs/{}", s.file), "heading": s.heading, "score": (score * 100.0).round() / 100.0, "text": text })
        })
        .collect();
    let found = results.len();
    rep.data = json!({ "query": query, "results": results });
    // The sections themselves are what a reader wants, in the text a shell prints.
    rep.summary = if found == 0 { format!("nothing in the docs matches '{query}'") } else { format!("{found} sections for '{query}'\n\n{}", as_text(&rep)) };
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}

/// The results as text to read: each section under its file and heading.
pub fn as_text(rep: &Report) -> String {
    let mut out = String::new();
    for r in rep.data.get("results").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        out.push_str(&format!("## {} > {}\n\n{}\n\n", r["file"].as_str().unwrap_or(""), r["heading"].as_str().unwrap_or(""), r["text"].as_str().unwrap_or("")));
    }
    out
}
