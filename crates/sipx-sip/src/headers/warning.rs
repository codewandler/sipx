//! RFC 3261 §20.43 Warning grammar with retained source ranges.

use std::ops::Range;

use bytes::Bytes;

use super::grammar::{is_token_char, quoted_string_end, split_list_spans};
use crate::error::HeaderError;
use crate::uri::Host;

/// One complete `warning-value` and the agent range retained by its parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WarningValueSpan {
    pub(crate) agent: Range<usize>,
}

/// Parse every comma-joined Warning value and retain each complete agent span.
pub(crate) fn value_spans(value: &[u8]) -> Result<Vec<WarningValueSpan>, HeaderError> {
    split_list_spans(value, "Warning")?
        .into_iter()
        .map(|part| value_span(value, part))
        .collect()
}

fn value_span(value: &[u8], part: Range<usize>) -> Result<WarningValueSpan, HeaderError> {
    let item = trimmed_range(value, part);
    let bytes = value
        .get(item.clone())
        .ok_or(HeaderError::Syntax { header: "Warning" })?;

    if bytes.len() < 3
        || !bytes
            .get(..3)
            .is_some_and(|code| code.iter().all(u8::is_ascii_digit))
    {
        return Err(HeaderError::Syntax { header: "Warning" });
    }
    if bytes.get(3) != Some(&b' ') {
        return Err(HeaderError::Syntax { header: "Warning" });
    }

    let agent_start = 4usize;
    let agent_end = bytes
        .get(agent_start..)
        .and_then(|tail| tail.iter().position(|byte| *byte == b' '))
        .and_then(|offset| agent_start.checked_add(offset))
        .ok_or(HeaderError::Syntax { header: "Warning" })?;
    let agent = bytes
        .get(agent_start..agent_end)
        .ok_or(HeaderError::Syntax { header: "Warning" })?;
    if !valid_agent(agent) {
        return Err(HeaderError::Syntax { header: "Warning" });
    }

    let text_start = agent_end
        .checked_add(1)
        .ok_or(HeaderError::Syntax { header: "Warning" })?;
    let text_end = quoted_string_end(bytes, text_start)
        .ok_or(HeaderError::UnterminatedQuotedString { header: "Warning" })?;
    if text_end != bytes.len() {
        return Err(HeaderError::Syntax { header: "Warning" });
    }

    let start = item
        .start
        .checked_add(agent_start)
        .ok_or(HeaderError::Syntax { header: "Warning" })?;
    let end = item
        .start
        .checked_add(agent_end)
        .ok_or(HeaderError::Syntax { header: "Warning" })?;
    Ok(WarningValueSpan { agent: start..end })
}

fn valid_agent(agent: &[u8]) -> bool {
    !agent.is_empty()
        && (agent.iter().copied().all(is_token_char)
            || Host::parse_hostport(&Bytes::copy_from_slice(agent)).is_ok())
}

fn trimmed_range(value: &[u8], mut range: Range<usize>) -> Range<usize> {
    while range.start < range.end && matches!(value.get(range.start), Some(b' ' | b'\t')) {
        range.start += 1;
    }
    while range.start < range.end
        && matches!(
            range.end.checked_sub(1).and_then(|end| value.get(end)),
            Some(b' ' | b'\t')
        )
    {
        range.end -= 1;
    }
    range
}
