//! Grants and the lock (docs/spec/server.md 3.6; shared/contract/mcp.md 3.1 and 14): a host
//! started with grants (`amoris serve <project> --grants <file>`) is locked. Each grant binds a
//! token to one role, a seat's player, a checker or a developer, and every route then needs a
//! registered token (`http/locked.rs`), so no client raises its role by asking: a player's calls
//! are its seat's whatever the request says, a checker's are the catalog's reads alone, a
//! developer's everything. A host that was never locked takes no token and serves as before.
//!
//! Tokens are the process owner's: 128 bits it draws and writes in the file as 32 hex digits. The
//! engine draws none. A token is compared in constant time, has no `Serialize` and prints nothing
//! of itself, so no row, event, answer, host file or log carries one.
//!
//! All of it is host state, like the sessions (`sessions.rs`): the grants, the lock and the rows
//! live in the server, which holds no game. Nothing here is in the world, its hash, a snapshot or
//! a recording; the game sees only the `Developer(n)` or `Player(i)` source a command comes from.
//! Rows open in the file's order when the host locks, so a grants file gives each developer and
//! checker grant the same source in every run.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use amoris_contract::{Candidate, CheckOptions, Problem, codes, detail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Host;
use crate::sessions::Role;

/// The longest label a grant keeps, in characters (the rows' limit, `sessions.rs`).
const LABEL_MAX: usize = 80;

/// Catalog entries of kind `read` that a checker is not sent, because they do more than read:
///
/// - `scripts.check` compiles through the caller's client as `scripts.apply {dry_run}` (a
///   Request), has the declarations written under `.amoris/types/` and starts `tsc`;
/// - `scripts.types` writes `.amoris/types/` and, in a project that has none, `tsconfig.json`
///   in the project's root.
///
/// By server.md 3's own kinds both are requests. The list goes when the catalog says so
/// (amoris-server's `server_catalog`, amoris-runtime's `catalog.rs`).
const CHECKER_WITHHELD: [&str; 2] = ["scripts.check", "scripts.types"];

/// A grant's token: the 128 bits its 32 hex digits spell.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Token([u8; 16]);

impl Token {
    /// The token `text` spells: exactly 32 hex digits, in either case. Anything else, text that
    /// is not ASCII included, is no token.
    fn parse(text: &str) -> Option<Token> {
        let bytes = text.as_bytes();
        if bytes.len() != 32 {
            return None;
        }
        let digit = |b: u8| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        };
        let mut out = [0u8; 16];
        for (byte, [high, low]) in out.iter_mut().zip(bytes.as_chunks::<2>().0) {
            *byte = (digit(*high)? << 4) | digit(*low)?;
        }
        Some(Token(out))
    }
}

impl fmt::Debug for Token {
    /// A token is never printed.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(..)")
    }
}

/// One grant: a token and what the client that presents it is.
#[derive(Clone, Debug)]
struct Grant {
    token: Token,
    role: Role,
    /// The seat a player grant plays; a checker's or a developer's has none.
    seat: Option<String>,
    /// What the grant's session is called in the list (`agent.session.list`).
    label: String,
    /// A player grant whose policy has `fork` (shared/contract/mcp.md 3.1): its session may
    /// look ahead with `player.fork`.
    fork: bool,
}

/// The grants a host is locked with, as the grants file gives them.
#[derive(Clone, Debug)]
pub struct Grants {
    list: Vec<Grant>,
    /// The file they were read from, for the refusals of the lock.
    file: Option<String>,
}

/// The grants file: `{"grants": [{"token", "role", "seat"?, "label"?, "policy"?}]}`. Strict: a
/// field it does not take (shared/contract/mcp.md 3.1's `profile`, `clock`, `decision`,
/// `max_wall_ms` and the policy's fields but `fork`, which nothing enforces yet) is refused, never
/// accepted and ignored.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct File {
    grants: Vec<Entry>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Entry {
    /// 32 hex digits: 128 bits the process owner drew.
    token: String,
    /// `player`, `checker` or `developer`.
    role: Role,
    /// The seat a player plays; required for a player, refused for the others.
    #[serde(default)]
    seat: Option<String>,
    /// The session's label in `agent.session.list`; default: the role's name.
    #[serde(default)]
    label: Option<String>,
    /// A player grant's policy; default: no lookahead.
    #[serde(default)]
    policy: Option<Policy>,
}

/// Of mcp.md 3.1's `Policy`, what a host enforces: whether a player may fork branches for
/// lookahead (docs/spec/player.md 7.1). Its bounds are the runtime's own.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Policy {
    /// May fork branches of main (`player.fork`); default false.
    #[serde(default)]
    fork: bool,
}

/// Why a grants file, or one of its grants, is refused. Never holds a token.
struct Refusal {
    grant: Option<usize>,
    label: Option<String>,
    field: Option<&'static str>,
    reason: String,
    /// The decoder's or the game's problem behind it (boxed: a refusal is an error value).
    problem: Option<Box<Problem>>,
}

impl Refusal {
    fn file(reason: impl Into<String>) -> Refusal {
        Refusal {
            grant: None,
            label: None,
            field: None,
            reason: reason.into(),
            problem: None,
        }
    }

    fn grant(index: usize, label: &str, field: &'static str, reason: impl Into<String>) -> Refusal {
        Refusal {
            grant: Some(index),
            label: Some(label.to_owned()),
            field: Some(field),
            reason: reason.into(),
            problem: None,
        }
    }

    fn because(mut self, problem: Problem) -> Refusal {
        self.problem = Some(Box::new(problem));
        self
    }

    /// `host.grants_invalid {file?, grant?, label?, field?, reason, problem?}`.
    fn problem(self, file: Option<&str>) -> Problem {
        let what = match file {
            Some(f) => format!("The grants file {f}"),
            None => "The grants file".to_owned(),
        };
        let message = match (self.grant, &self.label) {
            (Some(i), Some(l)) => {
                format!("{what} is refused at grant {i} ('{l}'): {}.", self.reason)
            }
            _ => format!("{what} is refused: {}.", self.reason),
        };
        let mut d = detail([("reason", json!(self.reason))]);
        if let Some(f) = file {
            d.insert("file".into(), json!(f));
        }
        if let Some(i) = self.grant {
            d.insert("grant".into(), json!(i));
        }
        if let Some(l) = self.label {
            d.insert("label".into(), json!(l));
        }
        if let Some(f) = self.field {
            d.insert("field".into(), json!(f));
        }
        if let Some(p) = self.problem {
            d.insert("problem".into(), json!(p));
        }
        Problem::new("host.grants_invalid", message, d)
    }
}

/// `session.seat_taken` (shared/contract/mcp.md 7.1): a seat two player sessions would hold.
fn seat_taken(seat: &str) -> Problem {
    Problem::new(
        "session.seat_taken",
        format!("Seat {seat} is already played by another session."),
        detail([("seat", json!(seat))]),
    )
}

fn parse(text: &str) -> Result<Vec<Grant>, Refusal> {
    // serde_json's syntax errors name a place, never the text.
    let value: Value =
        serde_json::from_str(text).map_err(|e| Refusal::file(format!("it is not JSON ({e})")))?;
    let file = amoris_contract::decode::<File>(&value, &CheckOptions::new("the grants file"))
        .map_err(|p| {
            // The decoder's own words (the field it does not take, with its suggestions).
            Refusal::file(p.message.trim_end_matches('.').to_owned()).because(p)
        })?
        .value;
    if file.grants.is_empty() {
        return Err(Refusal::file("it registers no grant"));
    }
    let mut list: Vec<Grant> = Vec::with_capacity(file.grants.len());
    for (i, entry) in file.grants.into_iter().enumerate() {
        let role = entry.role;
        let label: String = entry
            .label
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .unwrap_or(role.name())
            .chars()
            .take(LABEL_MAX)
            .collect();
        let Some(token) = Token::parse(&entry.token) else {
            return Err(Refusal::grant(
                i,
                &label,
                "token",
                "a token is exactly 32 hex digits, 128 bits the process owner draws",
            ));
        };
        if let Some(first) = list.iter().position(|g| g.token == token) {
            return Err(Refusal::grant(
                i,
                &label,
                "token",
                format!("it repeats the token of grant {first}"),
            ));
        }
        let seat = entry.seat.filter(|s| !s.is_empty());
        match (role, &seat) {
            (Role::Player, None) => {
                return Err(Refusal::grant(
                    i,
                    &label,
                    "seat",
                    "a player grant names the seat it plays",
                ));
            }
            (Role::Player, Some(seat)) => {
                if list.iter().any(|g| g.seat.as_deref() == Some(seat)) {
                    return Err(Refusal::grant(
                        i,
                        &label,
                        "seat",
                        format!("another player grant already plays the seat '{seat}'"),
                    )
                    .because(seat_taken(seat)));
                }
            }
            (_, Some(_)) => {
                return Err(Refusal::grant(
                    i,
                    &label,
                    "seat",
                    format!(
                        "only a player grant names a seat; a {} plays none",
                        role.name()
                    ),
                ));
            }
            (_, None) => {}
        }
        if entry.policy.is_some() && role != Role::Player {
            return Err(Refusal::grant(
                i,
                &label,
                "policy",
                format!(
                    "only a player grant has a policy; a {} {}",
                    role.name(),
                    if role == Role::Developer {
                        "may always fork"
                    } else {
                        "never forks"
                    }
                ),
            ));
        }
        let fork = entry.policy.is_some_and(|p| p.fork);
        list.push(Grant {
            token,
            role,
            seat,
            label,
            fork,
        });
    }
    Ok(list)
}

impl Grants {
    /// The grants a file's text registers, or `host.grants_invalid`: text that is not JSON, a
    /// field the file does not take, an empty list, a token that is not 32 hex digits or is
    /// given twice, a player without a seat, a seat on a checker or a developer, two players on
    /// one seat. No refusal says a token.
    pub fn parse(text: &str) -> Result<Grants, Problem> {
        match parse(text) {
            Ok(list) => Ok(Grants { list, file: None }),
            Err(r) => Err(r.problem(None)),
        }
    }

    /// [`Grants::parse`] of the file at `path`, which must lie outside `project`: a project's
    /// files are served at `/assets/<path>`, so a grants file inside it would hand every token
    /// to each checker and developer grant.
    pub fn load(path: &Path, project: &Path) -> Result<Grants, Problem> {
        let shown = path.display().to_string();
        let refused = |r: Refusal| r.problem(Some(&shown));
        let unreadable =
            |e: std::io::Error| refused(Refusal::file(format!("it cannot be read ({e})")));
        let full = path.canonicalize().map_err(unreadable)?;
        if let Ok(root) = project.canonicalize()
            && full.starts_with(&root)
        {
            return Err(refused(Refusal::file(
                "it lies inside the project, whose files the host serves; keep it outside",
            )));
        }
        let text = std::fs::read_to_string(&full).map_err(unreadable)?;
        match parse(&text) {
            Ok(list) => Ok(Grants {
                list,
                file: Some(shown),
            }),
            Err(r) => Err(refused(r)),
        }
    }

    /// How many grants each role has: players, checkers, developers.
    pub fn counts(&self) -> (usize, usize, usize) {
        let of = |role: Role| self.list.iter().filter(|g| g.role == role).count();
        (of(Role::Player), of(Role::Checker), of(Role::Developer))
    }
}

/// A grant and the row its calls are listed under.
struct Bound {
    grant: Grant,
    /// `mcp-<n>` (`agent.session.list`).
    session: String,
}

/// What a locked host holds: its grants in the file's order, and the checker's rule.
pub(crate) struct Lock {
    bound: Vec<Bound>,
    /// The catalog's reads, names and aliases, which a checker may send.
    reads: BTreeSet<String>,
    /// Every name and alias of the catalog.
    known: BTreeSet<String>,
}

/// The grant a request presented the token of. It holds no token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Granted {
    /// The grant's place in the file.
    pub index: usize,
    pub role: Role,
    pub seat: Option<String>,
    pub label: String,
    /// The row its calls are listed under (`mcp-<n>`).
    pub session: String,
}

/// The place of the token `presented` spells among `tokens`, if it is one of them. Text that is
/// no token finds nothing. Every byte of every token is compared, whatever differs and wherever
/// the match is, and the place is picked without a branch, so the time taken says nothing about
/// a registered token.
fn find<'a>(tokens: impl Iterator<Item = &'a Token>, presented: &str) -> Option<usize> {
    let presented = Token::parse(presented)?;
    let mut found = usize::MAX;
    for (i, token) in tokens.enumerate() {
        let mut differs = 0u8;
        for (a, b) in token.0.iter().zip(presented.0.iter()) {
            differs |= a ^ b;
        }
        // All ones when no byte differed, else zero.
        let same = usize::from(std::hint::black_box(differs) == 0).wrapping_neg();
        found = (i & same) | (found & !same);
    }
    (found != usize::MAX).then_some(found)
}

/// The catalog's reads a checker may send and every name it lists, aliases included.
fn kinds(catalog: &Value) -> (BTreeSet<String>, BTreeSet<String>) {
    let (mut reads, mut known) = (BTreeSet::new(), BTreeSet::new());
    for entry in catalog.as_array().into_iter().flatten() {
        let Some(name) = entry["name"].as_str() else {
            continue;
        };
        let read = entry["kind"] == json!("read") && !CHECKER_WITHHELD.contains(&name);
        let aliases = entry["aliases"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str);
        for n in std::iter::once(name).chain(aliases) {
            known.insert(n.to_owned());
            if read {
                reads.insert(n.to_owned());
            }
        }
    }
    (reads, known)
}

/// Whether a checker may send `method`: a read of the catalog. Another method the catalog lists
/// needs a developer; a name it does not list is no method, said with the reads a checker has.
fn permits(
    reads: &BTreeSet<String>,
    known: &BTreeSet<String>,
    method: &str,
) -> Result<(), Problem> {
    if reads.contains(method) {
        Ok(())
    } else if known.contains(method) {
        Err(codes::permission_denied(method, "checker", "developer"))
    } else {
        let valid: Vec<Candidate<'_>> = reads.iter().map(|r| Candidate::new(r)).collect();
        Err(codes::unknown_method(method, &valid, None))
    }
}

impl Host {
    /// Locks the host with `grants`: from now on every route needs a registered token. Called
    /// once, before the host serves ([`Host::serve`], [`crate::router`]). Each player grant's
    /// seat is asked of the game first (`host.grants_invalid` with the game's refusal when it
    /// declares no such seat or no players at all); then the grants' rows open in the file's
    /// order, live for the host's life.
    pub async fn lock(&self, grants: Grants) -> Result<(), Problem> {
        let file = grants.file.clone();
        let refused = |r: Refusal| r.problem(file.as_deref());
        match self.0.grants.get() {
            Some(Some(_)) => return Err(refused(Refusal::file("this host is locked already"))),
            Some(None) => {
                return Err(refused(Refusal::file(
                    "this host already serves without grants; a host is locked before it serves",
                )));
            }
            None => {}
        }
        let (reads, known) = kinds(&self.catalog().await?);
        let mut seats = Vec::with_capacity(grants.list.len());
        for (i, grant) in grants.list.iter().enumerate() {
            seats.push(match &grant.seat {
                Some(seat) => Some(self.player_client(seat).await.map_err(|p| {
                    refused(
                        Refusal::grant(
                            i,
                            &grant.label,
                            "seat",
                            format!("the game gives no player the seat '{seat}'"),
                        )
                        .because(p),
                    )
                })?),
                None => None,
            });
        }
        let mut bound = Vec::with_capacity(grants.list.len());
        for (grant, client) in grants.list.into_iter().zip(seats) {
            let session = self.grant_row(grant.role, grant.seat.as_deref(), &grant.label);
            if grant.role == Role::Player && !grant.fork {
                self.sessions().forbid_fork(&session);
            }
            if let Some(client) = client {
                self.grant_client(&session, client);
            }
            bound.push(Bound { grant, session });
        }
        let lock = Lock {
            bound,
            reads,
            known,
        };
        self.0
            .grants
            .set(Some(lock))
            .map_err(|_| refused(Refusal::file("this host was locked or served meanwhile")))
    }

    /// The lock, when the host has one.
    fn lock_held(&self) -> Option<&Lock> {
        self.0.grants.get().and_then(Option::as_ref)
    }

    /// Whether the host was started with grants.
    pub fn locked(&self) -> bool {
        self.lock_held().is_some()
    }

    /// Settles that a host which was not locked serves without grants: [`Host::lock`] is
    /// refused from now on, so no host says it is locked while routes built before answer
    /// without a token.
    pub(crate) fn serves_unlocked(&self) {
        self.0.grants.get_or_init(|| None);
    }

    /// The grant whose token `presented` is, compared with every grant's in constant time.
    pub(crate) fn granted(&self, presented: &str) -> Option<Granted> {
        let lock = self.lock_held()?;
        let index = find(lock.bound.iter().map(|b| &b.grant.token), presented)?;
        lock.bound.get(index).map(|b| b.granted(index))
    }

    /// Every grant, in the file's order; none on a host that is not locked.
    pub(crate) fn grants(&self) -> Vec<Granted> {
        self.lock_held().map_or_else(Vec::new, |lock| {
            lock.bound
                .iter()
                .enumerate()
                .map(|(i, b)| b.granted(i))
                .collect()
        })
    }

    /// Whether a checker may send `method`. A host that is not locked has no checker.
    pub(crate) fn checker_permits(&self, method: &str) -> Result<(), Problem> {
        match self.lock_held() {
            Some(lock) => permits(&lock.reads, &lock.known, method),
            None => Err(codes::permission_denied(method, "checker", "developer")),
        }
    }
}

impl Bound {
    fn granted(&self, index: usize) -> Granted {
        Granted {
            index,
            role: self.grant.role,
            seat: self.grant.seat.clone(),
            label: self.grant.label.clone(),
            session: self.session.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYER: &str = "0123456789abcdef0123456789abcdef";
    const CHECKER: &str = "fedcba9876543210fedcba9876543210";
    const DEVELOPER: &str = "00112233445566778899aabbccddeeff";

    fn file(grants: Value) -> String {
        json!({"grants": grants}).to_string()
    }

    /// A refusal of `text`, which must say none of `tokens` anywhere.
    fn refused(text: &str, tokens: &[&str]) -> Problem {
        let p = Grants::parse(text).expect_err(text);
        assert_eq!(p.code, "host.grants_invalid", "{p:#?}");
        let said = format!("{} {p:?}", json!(p));
        for t in tokens {
            assert!(!said.contains(t), "a refusal says a token: {said}");
            assert!(!said.contains(&t.to_uppercase()), "{said}");
        }
        p
    }

    #[test]
    fn a_grants_file_is_read_strictly() {
        let three = file(json!([
            {"token": PLAYER, "role": "player", "seat": "skipper", "label": "agent"},
            {"token": CHECKER, "role": "checker"},
            {"token": DEVELOPER.to_uppercase(), "role": "developer", "label": "  "},
        ]));
        let grants = Grants::parse(&three).unwrap();
        assert_eq!(grants.counts(), (1, 1, 1));
        let labels: Vec<&str> = grants.list.iter().map(|g| g.label.as_str()).collect();
        assert_eq!(labels, ["agent", "checker", "developer"]);
        assert_eq!(grants.list[0].seat.as_deref(), Some("skipper"));
        // Upper-case digits spell the same token.
        assert_eq!(Some(grants.list[2].token), Token::parse(DEVELOPER));
        // Nothing that prints the grants says a token.
        let printed = format!("{grants:?}");
        for t in [PLAYER, CHECKER, DEVELOPER] {
            assert!(!printed.contains(t), "{printed}");
            assert!(!printed.contains(&t.to_uppercase()), "{printed}");
        }
        // A label is cut to 80 characters, at a character's boundary.
        let long =
            file(json!([{"token": DEVELOPER, "role": "developer", "label": "é".repeat(200)}]));
        assert_eq!(
            Grants::parse(&long).unwrap().list[0].label.chars().count(),
            80
        );

        // What is no token: 31 and 33 digits, a digit that is not hex, text that is not ASCII.
        let short = &PLAYER[..31];
        let long = format!("{PLAYER}0");
        let not_hex = format!("{}g", &PLAYER[..31]);
        let not_ascii = format!("{}é", &PLAYER[..30]);
        assert_eq!(not_ascii.len(), 32);
        for token in [
            short,
            long.as_str(),
            not_hex.as_str(),
            not_ascii.as_str(),
            "",
        ] {
            let p = refused(
                &file(json!([{"token": token, "role": "developer"}])),
                &[token]
                    .into_iter()
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>(),
            );
            assert_eq!(
                (p.detail["grant"].clone(), p.detail["field"].clone()),
                (json!(0), json!("token")),
                "{p:#?}"
            );
        }
        // A token given twice, in either case.
        let p = refused(
            &file(json!([
                {"token": CHECKER, "role": "checker"},
                {"token": CHECKER.to_uppercase(), "role": "developer", "label": "setup"},
            ])),
            &[CHECKER],
        );
        assert_eq!(
            (p.detail["grant"].clone(), p.detail["label"].clone()),
            (json!(1), json!("setup")),
            "{p:#?}"
        );
        // Seats: a player has one, the others none, and no two players share one.
        let p = refused(
            &file(json!([{"token": PLAYER, "role": "player"}])),
            &[PLAYER],
        );
        assert_eq!(p.detail["field"], json!("seat"), "{p:#?}");
        for role in ["checker", "developer"] {
            let p = refused(
                &file(json!([{"token": PLAYER, "role": role, "seat": "skipper"}])),
                &[PLAYER],
            );
            assert_eq!(p.detail["field"], json!("seat"), "{p:#?}");
        }
        let p = refused(
            &file(json!([
                {"token": PLAYER, "role": "player", "seat": "skipper"},
                {"token": CHECKER, "role": "player", "seat": "skipper"},
            ])),
            &[PLAYER, CHECKER],
        );
        assert_eq!(
            p.detail["problem"]["code"],
            json!("session.seat_taken"),
            "{p:#?}"
        );
        assert_eq!(
            p.detail["problem"]["detail"]["seat"],
            json!("skipper"),
            "{p:#?}"
        );
        // A policy is a player's, and of it only `fork` is taken.
        for role in ["checker", "developer"] {
            let p = refused(
                &file(json!([{"token": PLAYER, "role": role, "policy": {"fork": true}}])),
                &[PLAYER],
            );
            assert_eq!(p.detail["field"], json!("policy"), "{p:#?}");
        }
        let p = refused(
            &file(
                json!([{"token": PLAYER, "role": "player", "seat": "skipper",
                          "policy": {"fork": true, "max_branches": 2}}]),
            ),
            &[PLAYER],
        );
        assert_eq!(
            p.detail["problem"]["code"],
            json!("request.unknown_field"),
            "{p:#?}"
        );
        let g = Grants::parse(&file(json!([
            {"token": PLAYER, "role": "player", "seat": "skipper", "policy": {"fork": true}},
            {"token": CHECKER, "role": "player", "seat": "crew"},
        ])))
        .unwrap();
        assert_eq!(
            g.list.iter().map(|g| g.fork).collect::<Vec<_>>(),
            [true, false]
        );
        // A field the file does not take, which a later grant form may: refused, never ignored.
        for field in ["profile", "clock", "decision", "max_wall_ms"] {
            let mut grant = json!({"token": PLAYER, "role": "developer"});
            grant[field] = json!({});
            let p = refused(&file(json!([grant])), &[PLAYER]);
            let inner = &p.detail["problem"];
            assert_eq!(
                inner["code"],
                json!("request.unknown_field"),
                "{field}: {p:#?}"
            );
            assert_eq!(
                inner["detail"]["path"],
                json!(format!("/grants/0/{field}")),
                "{p:#?}"
            );
        }
        let p = refused(&json!({"grants": [], "locked": true}).to_string(), &[]);
        assert_eq!(
            p.detail["problem"]["code"],
            json!("request.unknown_field"),
            "{p:#?}"
        );
        // A role that is none, an empty list, text that is not JSON, a file that is no object.
        let p = refused(
            &file(json!([{"token": PLAYER, "role": "admin"}])),
            &[PLAYER],
        );
        assert!(p.detail["problem"].is_object(), "{p:#?}");
        let p = refused(&file(json!([])), &[]);
        assert!(p.message.contains("no grant"), "{p:#?}");
        refused(
            &format!("{{\"grants\": [{{\"token\": \"{PLAYER}\","),
            &[PLAYER],
        );
        refused("[]", &[]);
        refused("", &[]);
    }

    #[test]
    fn a_grants_file_inside_the_project_is_refused() {
        let dir = std::env::temp_dir().join(format!("amoris-grants-{}", std::process::id()));
        let project = dir.join("project");
        std::fs::create_dir_all(project.join("notes")).unwrap();
        let text = file(json!([{"token": DEVELOPER, "role": "developer"}]));
        let inside = project.join("notes").join("grants.json");
        let outside = dir.join("grants.json");
        std::fs::write(&inside, &text).unwrap();
        std::fs::write(&outside, &text).unwrap();
        let p = Grants::load(&inside, &project).unwrap_err();
        assert_eq!(p.code, "host.grants_invalid", "{p:#?}");
        assert!(p.message.contains("inside the project"), "{p:#?}");
        assert_eq!(
            p.detail["file"],
            json!(inside.display().to_string()),
            "{p:#?}"
        );
        // The same file reached by a path that leaves the project and comes back.
        let around = project
            .join("..")
            .join("project")
            .join("notes")
            .join("grants.json");
        assert!(Grants::load(&around, &project).is_err());
        let grants = Grants::load(&outside, &project).unwrap();
        assert_eq!(grants.counts(), (0, 0, 1));
        assert_eq!(
            grants.file.as_deref(),
            Some(outside.display().to_string().as_str())
        );
        // A file that is not there, and one that is refused, name the file and no token.
        let p = Grants::load(&dir.join("none.json"), &project).unwrap_err();
        assert_eq!(p.code, "host.grants_invalid", "{p:#?}");
        std::fs::write(
            &outside,
            file(json!([{"token": DEVELOPER, "role": "player"}])),
        )
        .unwrap();
        let p = Grants::load(&outside, &project).unwrap_err();
        assert_eq!(
            p.detail["file"],
            json!(outside.display().to_string()),
            "{p:#?}"
        );
        assert!(!format!("{p:?}").contains(DEVELOPER), "{p:#?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tokens_are_found_whatever_differs() {
        let texts = [
            PLAYER,
            CHECKER,
            DEVELOPER,
            "ffffffffffffffffffffffffffffffff",
            "00000000000000000000000000000000",
        ];
        let tokens: Vec<Token> = texts.iter().map(|t| Token::parse(t).unwrap()).collect();
        for (i, text) in texts.iter().enumerate() {
            assert_eq!(find(tokens.iter(), text), Some(i), "{i}");
            assert_eq!(find(tokens.iter(), &text.to_uppercase()), Some(i), "{i}");
        }
        // One bit off a registered token, any of its 128, is no token of the host.
        for (i, token) in tokens.iter().enumerate() {
            for bit in 0..128 {
                let mut bytes = token.0;
                bytes[bit / 8] ^= 1 << (bit % 8);
                let flipped: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                assert_eq!(find(tokens.iter(), &flipped), None, "grant {i}, bit {bit}");
            }
        }
        // What is no token finds nothing, a grant of 32 zeros included.
        let long = format!("{PLAYER}00");
        for none in [
            "",
            "0",
            &PLAYER[..31],
            long.as_str(),
            "zz112233445566778899aabbccddeeff",
        ] {
            assert_eq!(find(tokens.iter(), none), None, "{none:?}");
        }
        let not_ascii = format!("{}é", &PLAYER[..30]);
        assert_eq!(find(tokens.iter(), &not_ascii), None);
        assert_eq!(find([].iter(), PLAYER), None);
    }

    #[test]
    fn a_checker_reads_what_the_catalog_calls_a_read() {
        let entry = |name: &str, kind: &str, aliases: &[&str]| json!({"name": name, "kind": kind, "aliases": aliases});
        let catalog = json!([
            entry("status", "read", &[]),
            entry("world.get", "read", &["world_get"]),
            entry("world.edit", "write", &[]),
            entry("world_edit", "write", &[]),
            entry("time.step", "control", &["step"]),
            entry("scripts.apply", "request", &[]),
            entry("scripts.check", "read", &[]),
            entry("scripts.types", "read", &[]),
            entry("scripts.profile", "read", &[]),
            entry("debug.eval", "write", &[]),
            entry("debug.state", "read", &[]),
            json!({"kind": "read"}),
        ]);
        let (reads, known) = kinds(&catalog);
        let reads_are: Vec<&str> = reads.iter().map(String::as_str).collect();
        assert_eq!(
            reads_are,
            [
                "debug.state",
                "scripts.profile",
                "status",
                "world.get",
                "world_get"
            ]
        );
        assert_eq!(known.len(), 13);
        for read in &reads {
            assert!(permits(&reads, &known, read).is_ok(), "{read}");
        }
        for other in [
            "world.edit",
            "world_edit",
            "time.step",
            "step",
            "scripts.apply",
            "scripts.check",
            "scripts.types",
            "debug.eval",
        ] {
            let p = permits(&reads, &known, other).unwrap_err();
            assert_eq!(p.code, "permission.denied", "{other}: {p:#?}");
            assert_eq!(
                (
                    p.detail["request"].clone(),
                    p.detail["role"].clone(),
                    p.detail["needs"].clone()
                ),
                (json!(other), json!("checker"), json!("developer")),
                "{p:#?}"
            );
        }
        let p = permits(&reads, &known, "world.gett").unwrap_err();
        assert_eq!(p.code, "request.unknown_method", "{p:#?}");
        assert_eq!(p.detail["suggestions"][0], json!("world.get"), "{p:#?}");
        // What it may send instead: the reads alone.
        assert_eq!(
            p.detail["allowed"].as_array().map(Vec::len),
            Some(reads.len())
        );
        // A catalog that is no list permits nothing.
        let (reads, known) = kinds(&json!({}));
        assert!(permits(&reads, &known, "status").is_err());
    }
}
