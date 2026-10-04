//! "Did you mean ...": the suggestion rule every place that names something uses
//! (shared/contract/errors.md, Unknown names and suggestions). Deterministic, so both lines give the
//! same suggestions in the same order.

use std::cmp::Ordering;

/// The unit suffixes of shared/contract/README.md, Names.
pub const UNIT_SUFFIXES: [&str; 5] = ["_m", "_mps", "_deg", "_s", "_kg"];

/// At most this many suggestions (errors.md, Codes).
pub const MAX_SUGGESTIONS: usize = 3;

/// A valid name and the aliases it declares; an alias that matches suggests the name.
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    pub name: &'a str,
    pub aliases: &'a [&'a str],
}

impl<'a> Candidate<'a> {
    pub fn new(name: &'a str) -> Candidate<'a> {
        Candidate { name, aliases: &[] }
    }

    pub fn with_aliases(name: &'a str, aliases: &'a [&'a str]) -> Candidate<'a> {
        Candidate { name, aliases }
    }
}

/// How close an unknown name is to a valid one: a lower score is a better suggestion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Score {
    /// The same name in another case or separator style.
    SameName = 0,
    /// The unit suffix left off or wrong.
    UnitSuffix = 1,
    /// A typo: an edit distance of at most a third of the length.
    Typo = 2,
    /// A shared word of three or more letters.
    RelatedWord = 3,
}

/// Normalization: `_` between a lowercase letter or digit and a following uppercase letter, ASCII
/// lowercase, `-`, space and `.` to `_`, runs of `_` collapsed, `_` trimmed at both ends.
pub fn normalize(name: &str) -> String {
    let mut spaced = String::with_capacity(name.len() + 4);
    let mut prev: Option<char> = None;
    for c in name.chars() {
        if let Some(p) = prev
            && (p.is_ascii_lowercase() || p.is_ascii_digit())
            && c.is_ascii_uppercase()
        {
            spaced.push('_');
        }
        spaced.push(c);
        prev = Some(c);
    }
    let mut out = String::with_capacity(spaced.len());
    for c in spaced.chars() {
        let c = match c {
            '-' | ' ' | '.' => '_',
            c => c.to_ascii_lowercase(),
        };
        if c == '_' && out.ends_with('_') {
            continue;
        }
        out.push(c);
    }
    out.trim_matches('_').to_owned()
}

/// A normalized name without a trailing unit suffix, or the name itself.
pub fn stem(normalized: &str) -> &str {
    for suffix in UNIT_SUFFIXES {
        if let Some(s) = normalized.strip_suffix(suffix)
            && !s.is_empty()
        {
            return s;
        }
    }
    normalized
}

/// The optimal-string-alignment distance (strsim 0.11.1).
pub fn osa(a: &str, b: &str) -> usize {
    strsim::osa_distance(a, b)
}

/// Scores the unknown name `unknown` against one valid spelling `valid` (a name or an alias), with
/// the distance that orders suggestions of scores 2 and 3. `None` when it is no suggestion.
pub fn score(unknown: &str, valid: &str) -> Option<(Score, usize)> {
    let nu = normalize(unknown);
    let nc = normalize(valid);
    if nu.is_empty() {
        return None;
    }
    if nu == nc {
        return Some((Score::SameName, 0));
    }
    if nu == stem(&nc) || stem(&nu) == stem(&nc) {
        return Some((Score::UnitSuffix, 0));
    }
    let len = nu.chars().count();
    let d = osa(&nu, &nc).min(osa(&nu, stem(&nc)));
    if d <= (len / 3).max(1) {
        return Some((Score::Typo, d));
    }
    let words = |s: &str| -> Vec<String> {
        s.split('_')
            .filter(|w| w.chars().count() >= 3 && !matches!(*w, "deg" | "mps"))
            .map(str::to_owned)
            .collect()
    };
    let wu = words(&nu);
    if words(&nc).iter().any(|w| wu.contains(w)) {
        return Some((Score::RelatedWord, osa(&nu, &nc)));
    }
    None
}

/// The best score of `unknown` against a candidate's name and aliases.
pub fn score_candidate(unknown: &str, c: &Candidate<'_>) -> Option<(Score, usize)> {
    std::iter::once(c.name)
        .chain(c.aliases.iter().copied())
        .filter_map(|v| score(unknown, v))
        .min()
}

/// The suggestions for `unknown` among `candidates`: ordered by score, then distance (scores 2 and
/// 3), then name bytes; without duplicates; at most three.
pub fn suggest<'a>(
    unknown: &str,
    candidates: impl IntoIterator<Item = Candidate<'a>>,
) -> Vec<String> {
    let mut scored: Vec<(Score, usize, &str)> = Vec::new();
    for c in candidates {
        if let Some((s, d)) = score_candidate(unknown, &c) {
            match scored.iter_mut().find(|e| e.2 == c.name) {
                Some(e) if (s, d) < (e.0, e.1) => *e = (s, d, c.name),
                Some(_) => {}
                None => scored.push((s, d, c.name)),
            }
        }
    }
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then_with(|| a.2.as_bytes().cmp(b.2.as_bytes()))
    });
    scored
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|e| e.2.to_owned())
        .collect()
}

/// [`suggest`] over plain names.
pub fn suggest_names<'a>(unknown: &str, names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    suggest(unknown, names.into_iter().map(Candidate::new))
}

/// The best candidate scoring 0 or 1 (the same name, or a unit suffix away): what misplaced-field
/// detection looks for one level away (errors.md, Misplaced fields).
pub fn close_match<'a>(
    unknown: &str,
    candidates: impl IntoIterator<Item = Candidate<'a>>,
) -> Option<&'a str> {
    let mut best: Option<(Score, &str)> = None;
    for c in candidates {
        if let Some((s, _)) = score_candidate(unknown, &c)
            && s <= Score::UnitSuffix
        {
            let better = match best {
                None => true,
                Some((bs, bn)) => match s.cmp(&bs) {
                    Ordering::Less => true,
                    Ordering::Equal => c.name.as_bytes() < bn.as_bytes(),
                    Ordering::Greater => false,
                },
            };
            if better {
                best = Some((s, c.name));
            }
        }
    }
    best.map(|b| b.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMS: [&str; 6] = [
        "heading_deg",
        "tolerance_deg",
        "turn",
        "settle_s",
        "keep",
        "timeout_s",
    ];
    const INTENTS: [&str; 5] = [
        "come_to_heading",
        "trim_sail",
        "sail_to",
        "take_aboard",
        "hold_course",
    ];
    const CONTROLS: [&str; 4] = ["rudder", "sheet", "hoist", "interact"];

    #[test]
    fn normalization() {
        assert_eq!(normalize("headingDeg"), "heading_deg");
        assert_eq!(normalize("Heading-Deg"), "heading_deg");
        assert_eq!(normalize("heading.deg"), "heading_deg");
        assert_eq!(normalize("__heading  deg__"), "heading_deg");
        assert_eq!(normalize("Mark9"), "mark9");
        assert_eq!(normalize("speed2X"), "speed2_x");
        assert_eq!(stem("heading_deg"), "heading");
        assert_eq!(stem("turn"), "turn");
        assert_eq!(stem("_m"), "_m");
    }

    #[test]
    fn the_contract_cases() {
        assert_eq!(suggest_names("headng", PARAMS), ["heading_deg"]);
        assert_eq!(suggest_names("headingDeg", PARAMS), ["heading_deg"]);
        assert_eq!(
            score("headingDeg", "heading_deg"),
            Some((Score::SameName, 0))
        );
        assert_eq!(suggest_names("heading_rad", PARAMS), ["heading_deg"]);
        assert_eq!(
            score("heading_rad", "heading_deg").map(|s| s.0),
            Some(Score::Typo)
        );
        assert_eq!(
            suggest_names("sail_toward", INTENTS),
            ["sail_to", "trim_sail"]
        );
        assert_eq!(suggest_names("come_to", INTENTS), ["come_to_heading"]);
        assert_eq!(suggest_names("ruder", CONTROLS), ["rudder"]);
        assert!(suggest_names("tiller", CONTROLS).is_empty());
        assert!(suggest_names("steer", CONTROLS).is_empty());
        assert_eq!(suggest_names("Mark9", ["Mark1", "Buoy3"]), ["Mark1"]);
        assert_eq!(score("mark1", "Mark1").map(|s| s.0), Some(Score::SameName));
        assert_eq!(
            suggest_names("Port", ["shortest", "port", "starboard"]),
            ["port"]
        );
        assert!(suggest_names("left", ["shortest", "port", "starboard"]).is_empty());
        assert_eq!(suggest_names("best_trim", ["best", "hold"]), ["best"]);
        assert_eq!(
            score("best_trim", "best").map(|s| s.0),
            Some(Score::RelatedWord)
        );
        assert_eq!(
            suggest_names("observ", ["observe", "act", "step"]),
            ["observe"]
        );
        assert_eq!(suggest_names("speed_m", ["speed_mps"]), ["speed_mps"]);
        assert_eq!(
            score("heading", "heading_deg").map(|s| s.0),
            Some(Score::UnitSuffix)
        );
    }

    #[test]
    fn typo_threshold_is_a_third_of_the_length() {
        // len 6: up to 2 edits suggest, 3 do not.
        assert_eq!(score("abcdef", "abcdxy").map(|s| s.0), Some(Score::Typo));
        assert_eq!(score("abcdef", "abcxyz"), None);
        // Short names: one edit always suggests.
        assert_eq!(score("ab", "ax").map(|s| s.0), Some(Score::Typo));
        assert_eq!(score("ab", "xy"), None);
        // Transposition costs one under OSA.
        assert_eq!(score("haeding", "heading").map(|s| s.1), Some(1));
    }

    #[test]
    fn ordering_ties_and_cap() {
        // Equal score and distance: by name bytes.
        assert_eq!(
            suggest_names("cat", ["cbt", "cas", "car"]),
            ["car", "cas", "cbt"]
        );
        // Score before distance; at most three.
        let s = suggest_names("speed", ["speed_mps", "sped", "speedo", "spend", "spee"]);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0], "speed_mps");
        // Aliases suggest their name, once.
        let c = [Candidate::with_aliases("rudder", &["tiller", "helm"])];
        assert_eq!(suggest("tiler", c), ["rudder"]);
        assert_eq!(close_match("helm", c), Some("rudder"));
        assert_eq!(close_match("helmet", c), None);
    }
}
