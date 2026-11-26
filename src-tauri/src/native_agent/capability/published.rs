//! What an Agent publishes to a collaboration at the end of its turn: the
//! fenced `pastey-message` and `pastey-summary` blocks that end its final
//! reply (`docs/research/agent-collaboration-links.md`).
//!
//! Only blocks at the very end of the reply count, so a block an Agent
//! quotes in its prose is never taken for one it sent. The sender is never
//! read from a block: it is the task Pastey ran. This module checks only
//! the blocks' own form; whether `to` names a member is the collaboration's
//! decision.

use serde::{Deserialize, Serialize};

use super::renders_faithfully;

pub(crate) const MAX_PUBLISHED_MESSAGES: usize = 3;
/// A message's body (after its subject line). Small enough that every block
/// a turn may publish fits one task output with room to spare.
pub(crate) const MAX_MESSAGE_BODY_BYTES: usize = 4 * 1024;
pub(crate) const MAX_SUMMARY_BYTES: usize = 2 * 1024;
pub(crate) const MAX_SUBJECT_CHARS: usize = 200;
const MAX_ALIAS_CHARS: usize = 32;

const MESSAGE_FENCE: &str = "```pastey-message";
const SUMMARY_FENCE: &str = "```pastey-summary";
const WAIT_FENCE: &str = "```pastey-wait";
const PLAN_FENCE: &str = "```pastey-plan";
/// A plan block's bound: a whole proposal of up to 30 tasks.
pub(crate) const MAX_PLAN_BYTES: usize = 6 * 1024;
/// A declared wait lasts at least this and at most a day (minutes).
pub(crate) const MIN_WAIT_MINUTES: u32 = 5;
pub(crate) const MAX_WAIT_MINUTES: u32 = 24 * 60;
pub(crate) const DEFAULT_WAIT_MINUTES: u32 = 120;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishedMessageV1 {
    /// A member alias or `user`, as the Agent wrote it.
    pub(crate) to: String,
    pub(crate) subject: String,
    pub(crate) body: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishedSummaryV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) done: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) decisions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) open: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) next: Option<String>,
}

/// A member declaring that it cannot go on until `on` (a member or `user`)
/// answers or delivers `reason`; it ends its turn and waits at most
/// `minutes` (R3 liveness).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishedWaitV1 {
    pub(crate) on: String,
    pub(crate) reason: String,
    pub(crate) minutes: u32,
}

/// A `pastey-plan` block as the Agent wrote it (R5): the collaboration
/// reads it; here it is only bounded and checked for faithful rendering.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishedPlanV1 {
    pub(crate) info: String,
    pub(crate) body: String,
}

/// The blocks one final reply published, and why any were not taken.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishedBlocksV1 {
    pub(crate) messages: Vec<PublishedMessageV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) summary: Option<PublishedSummaryV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) wait: Option<PublishedWaitV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) plan: Option<PublishedPlanV1>,
    /// Plain-language reasons, one per block (or rule) that was refused.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) problems: Vec<String>,
}

impl PublishedBlocksV1 {
    pub(crate) fn is_empty(&self) -> bool {
        self.messages.is_empty()
            && self.summary.is_none()
            && self.wait.is_none()
            && self.plan.is_none()
            && self.problems.is_empty()
    }
}

/// One fenced block found in the reply, by byte range.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KindV1 {
    Message,
    Summary,
    Wait,
    Plan,
}

struct BlockV1<'a> {
    start: usize,
    end: usize,
    kind: KindV1,
    info: &'a str,
    body: &'a str,
}

/// The length of the backtick run that starts `line`.
fn backticks(line: &str) -> usize {
    line.bytes().take_while(|&b| b == b'`').count()
}

/// A block being read: where it starts, its kind and info, how many
/// backticks opened it, and how many fenced code blocks inside it are open.
struct OpenBlockV1<'a> {
    start: usize,
    body_start: usize,
    kind: KindV1,
    info: &'a str,
    fence: usize,
    nested: usize,
}

/// Every `pastey-*` block that opens at the start of a line with three or
/// more backticks and closes with a line of only backticks.
///
/// A body may quote fenced code (P01: a message carrying a source file was
/// cut at the code's closing fence and dropped). With an opening fence of
/// four or more backticks, only a line of at least that many closes the
/// block. With three, a fence that names a language (```` ```python ````)
/// opens a nested block, and the bare ```` ``` ```` after it closes that,
/// not the message.
fn blocks(text: &str) -> Vec<BlockV1<'_>> {
    let mut found = Vec::new();
    let mut offset = 0;
    let mut open: Option<OpenBlockV1<'_>> = None;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let bare = line.trim_end_matches(['\n', '\r']);
        let ticks = backticks(bare);
        match open.as_mut() {
            None => {
                if ticks < 3 {
                    continue;
                }
                let tagged = &bare[ticks - 3..];
                let (kind, rest) = if let Some(rest) = tagged.strip_prefix(MESSAGE_FENCE) {
                    (KindV1::Message, rest)
                } else if let Some(rest) = tagged.strip_prefix(SUMMARY_FENCE) {
                    (KindV1::Summary, rest)
                } else if let Some(rest) = tagged.strip_prefix(WAIT_FENCE) {
                    (KindV1::Wait, rest)
                } else if let Some(rest) = tagged.strip_prefix(PLAN_FENCE) {
                    (KindV1::Plan, rest)
                } else {
                    continue;
                };
                // `pastey-messages` or similar is not this fence.
                if rest.is_empty() || rest.starts_with([' ', '\t']) {
                    open = Some(OpenBlockV1 {
                        start,
                        body_start: offset,
                        kind,
                        info: rest.trim(),
                        fence: ticks,
                        nested: 0,
                    });
                }
            }
            Some(block) => {
                let only_ticks = ticks >= 3 && bare[ticks..].trim().is_empty();
                let closes = if block.fence > 3 {
                    only_ticks && ticks >= block.fence
                } else if only_ticks && block.nested > 0 {
                    block.nested -= 1;
                    false
                } else if only_ticks {
                    true
                } else {
                    if ticks == 3 {
                        // A fence with a language opens quoted code.
                        block.nested += 1;
                    }
                    false
                };
                if closes {
                    let body = text[block.body_start..start].trim_end_matches(['\n', '\r']);
                    found.push(BlockV1 {
                        start: block.start,
                        end: offset,
                        kind: block.kind,
                        info: block.info,
                        body,
                    });
                    open = None;
                }
            }
        }
    }
    found
}

/// The run of blocks that ends the reply: each followed only by whitespace
/// up to the next block, and the last by whitespace to the end.
fn trailing<'a>(text: &str, found: Vec<BlockV1<'a>>) -> Vec<BlockV1<'a>> {
    let mut tail: Vec<BlockV1<'a>> = Vec::new();
    let mut after = text.len();
    for block in found.into_iter().rev() {
        if !text[block.end..after].trim().is_empty() {
            break;
        }
        after = block.start;
        tail.push(block);
    }
    tail.reverse();
    tail
}

/// Where the blocks that end `text` begin, if it ends with any.
pub(super) fn trailing_blocks_start(text: &str) -> Option<usize> {
    trailing(text, blocks(text)).first().map(|block| block.start)
}

fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.chars().count() <= MAX_ALIAS_CHARS
        && alias
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn message(block: &BlockV1<'_>) -> Result<PublishedMessageV1, String> {
    let to = block
        .info
        .split_whitespace()
        .find_map(|part| part.strip_prefix("to="))
        .ok_or("a message names no recipient (`to=`)")?;
    if !valid_alias(to) {
        return Err(format!("a message's recipient \"{to}\" is not a member alias"));
    }
    let (first, rest) = block.body.split_once('\n').unwrap_or((block.body, ""));
    let subject = first
        .strip_prefix("subject:")
        .map(str::trim)
        .filter(|subject| !subject.is_empty())
        .ok_or("a message has no `subject:` line")?;
    if subject.chars().count() > MAX_SUBJECT_CHARS {
        return Err(format!("a message's subject is over {MAX_SUBJECT_CHARS} characters"));
    }
    let body = rest.trim_matches(['\n', '\r']).to_owned();
    if body.trim().is_empty() {
        return Err("a message has no body".into());
    }
    if body.len() > MAX_MESSAGE_BODY_BYTES {
        return Err(format!(
            "a message to {to} is {} bytes, over {MAX_MESSAGE_BODY_BYTES}",
            body.len()
        ));
    }
    Ok(PublishedMessageV1 {
        to: to.to_owned(),
        subject: subject.to_owned(),
        body,
    })
}

/// The field a summary line starts, accepting the light decoration Agents
/// add ("- done:", "**done:**").
fn summary_field(line: &str) -> Option<(&'static str, &str)> {
    let line = line.trim_start().trim_start_matches(['-', '*', '\u{2022}']).trim_start();
    let line = line.trim_start_matches('*');
    for field in ["done", "decisions", "open", "next"] {
        let Some(rest) = line.get(..field.len()) else {
            continue;
        };
        if !rest.eq_ignore_ascii_case(field) {
            continue;
        }
        let rest = line[field.len()..].trim_start_matches('*');
        if let Some(value) = rest.strip_prefix(':') {
            return Some((field, value.trim_start_matches('*').trim()));
        }
    }
    None
}

fn summary(block: &BlockV1<'_>) -> Result<PublishedSummaryV1, String> {
    if !block.info.is_empty() {
        return Err("a summary block takes no attributes".into());
    }
    if block.body.len() > MAX_SUMMARY_BYTES {
        return Err(format!(
            "a summary is {} bytes, over {MAX_SUMMARY_BYTES}",
            block.body.len()
        ));
    }
    let mut fields: Vec<(&'static str, String)> = Vec::new();
    for line in block.body.lines() {
        if let Some((field, value)) = summary_field(line) {
            fields.push((field, value.to_owned()));
        } else if let Some((_, value)) = fields.last_mut() {
            // A continuation of the field above.
            if !line.trim().is_empty() {
                if !value.is_empty() {
                    value.push('\n');
                }
                value.push_str(line.trim());
            }
        }
    }
    let mut summary = PublishedSummaryV1::default();
    for (field, value) in fields {
        if value.is_empty() {
            continue;
        }
        let slot = match field {
            "done" => &mut summary.done,
            "decisions" => &mut summary.decisions,
            "open" => &mut summary.open,
            _ => &mut summary.next,
        };
        *slot = Some(match slot.take() {
            Some(earlier) => format!("{earlier}\n{value}"),
            None => value,
        });
    }
    if summary == PublishedSummaryV1::default() {
        return Err("a summary has none of done, decisions, open or next".into());
    }
    Ok(summary)
}

/// A duration such as `30m`, `2h` or `1d`, in minutes.
fn minutes(text: &str) -> Option<u32> {
    let text = text.trim();
    let (number, unit) = text.split_at(text.find(|c: char| !c.is_ascii_digit())?);
    let number: u32 = number.parse().ok()?;
    let factor = match unit.trim() {
        "m" | "min" | "mins" | "minutes" => 1,
        "h" | "hour" | "hours" => 60,
        "d" | "day" | "days" => 24 * 60,
        _ => return None,
    };
    number.checked_mul(factor)
}

fn wait(block: &BlockV1<'_>) -> Result<PublishedWaitV1, String> {
    let on = block
        .info
        .split_whitespace()
        .find_map(|part| part.strip_prefix("on="))
        .ok_or("a wait names nobody to wait on (`on=`)")?;
    if !valid_alias(on) {
        return Err(format!("a wait's \"{on}\" is not a member alias"));
    }
    let mut reason = None;
    let mut until = None;
    for line in block.body.lines() {
        let line = line.trim().trim_start_matches(['-', '*']).trim();
        if let Some(value) = line.strip_prefix("for:") {
            reason = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("until:") {
            until = Some(value.trim().to_owned());
        }
    }
    let reason = reason
        .filter(|reason| !reason.is_empty())
        .ok_or("a wait has no `for:` line saying what it needs")?;
    if reason.chars().count() > MAX_SUBJECT_CHARS {
        return Err(format!("a wait's `for:` is over {MAX_SUBJECT_CHARS} characters"));
    }
    let minutes = match until {
        None => DEFAULT_WAIT_MINUTES,
        Some(text) => minutes(&text)
            .ok_or_else(|| format!("a wait's `until: {text}` is not a duration like 30m, 2h or 1d"))?
            .clamp(MIN_WAIT_MINUTES, MAX_WAIT_MINUTES),
    };
    Ok(PublishedWaitV1 { on: on.to_owned(), reason, minutes })
}

/// The blocks that end `text`, checked against the block rules. Blocks that
/// break a rule are left out, each with its reason.
pub(crate) fn extract(text: &str) -> PublishedBlocksV1 {
    let found = blocks(text);
    let total = found.len();
    let tail = trailing(text, found);
    let mut published = PublishedBlocksV1::default();
    if tail.len() < total {
        published
            .problems
            .push("a block that was not at the end of the reply was ignored, so nothing in it was delivered".into());
    }
    for block in &tail {
        if !renders_faithfully(block.info) || !renders_faithfully(block.body) {
            published
                .problems
                .push("a block contains characters that could hide what it says".into());
            continue;
        }
        if block.kind == KindV1::Plan {
            if published.plan.is_some() {
                published.problems.push("a reply may carry only one plan block; later ones were ignored".into());
            } else if block.body.len() > MAX_PLAN_BYTES {
                published.problems.push(format!("a plan block is {} bytes, over {MAX_PLAN_BYTES}", block.body.len()));
            } else {
                published.plan = Some(PublishedPlanV1 { info: block.info.to_owned(), body: block.body.to_owned() });
            }
            continue;
        }
        if block.kind == KindV1::Wait {
            if published.wait.is_some() {
                published
                    .problems
                    .push("a reply may declare only one wait; later ones were ignored".into());
                continue;
            }
            match wait(block) {
                Ok(wait) => published.wait = Some(wait),
                Err(problem) => published.problems.push(problem),
            }
        } else if block.kind == KindV1::Summary {
            if published.summary.is_some() {
                published
                    .problems
                    .push("a reply may carry only one summary; later ones were ignored".into());
                continue;
            }
            match summary(block) {
                Ok(summary) => published.summary = Some(summary),
                Err(problem) => published.problems.push(problem),
            }
        } else if published.messages.len() == MAX_PUBLISHED_MESSAGES {
            published.problems.push(format!(
                "a reply may carry at most {MAX_PUBLISHED_MESSAGES} messages; later ones were ignored"
            ));
        } else {
            match message(block) {
                Ok(message) => published.messages.push(message),
                Err(problem) => published.problems.push(problem),
            }
        }
    }
    published
}

#[cfg(test)]
mod tests {
    use super::*;

    const MESSAGE: &str = "```pastey-message to=web\nsubject: Orders API now uses cursors\n\nOrdersTable still sends offset.\n```";

    #[test]
    fn a_wait_is_one_bounded_block_at_the_end() {
        let text = format!("Done for now.\n\n{MESSAGE}\n\n```pastey-wait on=web\nfor: the crate split compiles on your branch\nuntil: 3h\n```\n");
        let blocks = extract(&text);
        assert_eq!(blocks.messages.len(), 1);
        assert_eq!(
            blocks.wait,
            Some(PublishedWaitV1 { on: "web".into(), reason: "the crate split compiles on your branch".into(), minutes: 180 })
        );
        let long = extract("```pastey-wait on=web\nfor: x\nuntil: 9d\n```");
        assert_eq!(long.wait.unwrap().minutes, MAX_WAIT_MINUTES);
        let default = extract("```pastey-wait on=user\nfor: which phone first\n```");
        assert_eq!(default.wait.unwrap().minutes, DEFAULT_WAIT_MINUTES);
        assert!(extract("```pastey-wait\nfor: x\n```").problems[0].contains("on="));
        assert!(extract("```pastey-wait on=web\nuntil: 1h\n```").problems[0].contains("for:"));
        assert!(extract("```pastey-wait on=web\nfor: x\nuntil: soon\n```").problems[0].contains("duration"));
        let two = extract("```pastey-wait on=web\nfor: a\n```\n```pastey-wait on=api\nfor: b\n```");
        assert_eq!(two.wait.unwrap().on, "web");
        assert!(two.problems[0].contains("only one wait"));
    }
    const SUMMARY: &str = "```pastey-summary\ndone: reviewed pagination\nopen: web still uses offset\n```";

    #[test]
    fn a_message_and_a_summary_that_end_the_reply_are_published() {
        let text = format!("Found it.\n\n{MESSAGE}\n\n{SUMMARY}\n");
        let published = extract(&text);
        assert_eq!(published.messages.len(), 1);
        assert_eq!(published.messages[0].to, "web");
        assert_eq!(published.messages[0].subject, "Orders API now uses cursors");
        assert_eq!(published.messages[0].body, "OrdersTable still sends offset.");
        let summary = published.summary.unwrap();
        assert_eq!(summary.done.as_deref(), Some("reviewed pagination"));
        assert_eq!(summary.open.as_deref(), Some("web still uses offset"));
        assert!(published.problems.is_empty());
        assert_eq!(trailing_blocks_start(&text), Some("Found it.\n\n".len()));
    }

    #[test]
    fn kind_is_accepted_and_ignored() {
        let text = MESSAGE.replace("to=web", "to=web kind=info");
        assert_eq!(extract(&text).messages[0].to, "web");
    }

    #[test]
    fn a_block_followed_by_prose_is_not_published() {
        let published = extract(&format!("{MESSAGE}\n\nThanks!"));
        assert!(published.messages.is_empty());
        assert_eq!(published.problems.len(), 1);
        assert_eq!(trailing_blocks_start(&format!("{MESSAGE}\n\nThanks!")), None);
    }

    #[test]
    fn a_reply_without_blocks_publishes_nothing() {
        assert!(extract("GET /health returns status and version.").is_empty());
        assert!(extract("He wrote ```pastey-message to=web``` inline.").is_empty());
    }

    #[test]
    fn decorated_summary_fields_and_continuations_are_read() {
        let text = "x\n\n```pastey-summary\n- **done:** a\n  and b\n- next: c\n```";
        let summary = extract(text).summary.unwrap();
        assert_eq!(summary.done.as_deref(), Some("a\nand b"));
        assert_eq!(summary.next.as_deref(), Some("c"));
    }

    #[test]
    fn broken_blocks_are_refused_with_a_reason() {
        let cases = [
            MESSAGE.replace("to=web", "to=Web Team"),
            MESSAGE.replace("subject: Orders API now uses cursors\n", ""),
            MESSAGE.replace("to=web", ""),
            "x\n\n```pastey-summary\nAll good.\n```".to_owned(),
            MESSAGE.replace("OrdersTable", "Orders\u{202e}Table"),
        ];
        for text in cases {
            let published = extract(&text);
            assert!(published.messages.is_empty() && published.summary.is_none(), "{text}");
            assert_eq!(published.problems.len(), 1, "{text}");
        }
    }

    #[test]
    fn bounds_hold() {
        let long_body = "a".repeat(MAX_MESSAGE_BODY_BYTES + 1);
        let text = format!("```pastey-message to=web\nsubject: s\n\n{long_body}\n```");
        assert_eq!(extract(&text).problems.len(), 1);
        let four = [MESSAGE; 4].join("\n\n");
        let published = extract(&four);
        assert_eq!(published.messages.len(), MAX_PUBLISHED_MESSAGES);
        assert_eq!(published.problems.len(), 1);
        let two = format!("{SUMMARY}\n{SUMMARY}");
        let published = extract(&two);
        assert!(published.summary.is_some());
        assert_eq!(published.problems.len(), 1);
    }

    #[test]
    fn a_long_reply_keeps_its_blocks_whole_when_cut() {
        use super::super::{OpaqueCapabilityPayloadV1, MAX_CAPABILITY_PAYLOAD_BYTES};
        let text = format!("{}\n\n{MESSAGE}\n\n{SUMMARY}\n", "prose ".repeat(5000));
        let output = OpaqueCapabilityPayloadV1::text_output(&text).unwrap();
        assert!(serde_json::to_vec(&output.0).unwrap().len() <= MAX_CAPABILITY_PAYLOAD_BYTES);
        assert_eq!(output.0["truncated"], true);
        let published = output.published_blocks().unwrap();
        assert_eq!(published.messages.len(), 1);
        assert!(published.summary.is_some());
        assert!(published.problems.is_empty());
        // Without blocks, a long reply is cut at its end, as before.
        let plain = OpaqueCapabilityPayloadV1::text_output(&"prose ".repeat(5000)).unwrap();
        assert!(plain.published_blocks().is_none());
        assert!(plain.0["text"].as_str().unwrap().starts_with("prose prose"));
    }

    #[test]
    fn a_user_alias_is_a_recipient_like_any_other() {
        let text = MESSAGE.replace("to=web", "to=user");
        assert_eq!(extract(&text).messages[0].to, "user");
    }

    #[test]
    fn a_message_may_quote_fenced_code() {
        // P01: a message carrying a source file was cut at the code's own
        // closing fence, and the rest dropped as "not at the end".
        let text = "Reproduced.\n\n```pastey-message to=codex\nsubject: My sources\n\nThe gadget:\n\n```python\nclass Gen2(Gen):\n    pass\n```\n\n`stage2.py` is the same schedule.\n```\n\n```pastey-summary\ndone: sent sources\n```\n";
        let blocks = extract(text);
        assert!(blocks.problems.is_empty(), "{:?}", blocks.problems);
        assert_eq!(blocks.messages.len(), 1);
        assert!(blocks.messages[0].body.contains("```python\nclass Gen2(Gen):"));
        assert!(blocks.messages[0].body.ends_with("`stage2.py` is the same schedule."));
        assert_eq!(blocks.summary.unwrap().done.as_deref(), Some("sent sources"));
    }

    #[test]
    fn a_longer_fence_closes_only_on_as_many_backticks() {
        let text = "````pastey-message to=web\nsubject: Snippet\n\n```\nplain fence\n```\n````\n";
        let blocks = extract(text);
        assert!(blocks.problems.is_empty(), "{:?}", blocks.problems);
        assert_eq!(blocks.messages[0].body, "```\nplain fence\n```");
        // Three backticks never open a block that only four may close.
        assert!(extract("````pastey-message to=web\nsubject: x\n\nbody\n```\n").messages.is_empty());
    }
}
