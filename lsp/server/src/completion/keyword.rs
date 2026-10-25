//! Completes the keywords that the syntax allows at the cursor.
//!
//! The word being typed is parsed as a syntax error, and the keywords are the
//! ones its diagnostic would list as expected, before a label such as
//! "expected expression" stands in for them.

use rayc_lexical::kind::Keyword;
use rayc_qbice::TrackedEngine;
use rayc_source_file::ByteIndex;
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind};

use crate::{completion::path::Rank, site::File};

/// The keywords a path starts with. Completing a path offers them where
/// they name something, such as `super` outside of the root module.
const PATH_KEYWORDS: [Keyword; 3] = [Keyword::Target, Keyword::Super, Keyword::This];

/// Lists the keywords that can be written at `offset` in `file`, sorted by
/// name, except the ones labelled in `offered`.
pub(super) fn complete(
    engine: &TrackedEngine,
    file: &File,
    offset: ByteIndex,
    offered: &[CompletionItem],
) -> Vec<CompletionItem> {
    let mut items = file
        .expecteds_at(engine, offset)
        .into_iter()
        .filter_map(|expected| expected.into_keyword().ok())
        .filter(|keyword| !PATH_KEYWORDS.contains(keyword))
        .map(|keyword| keyword.to_string())
        .filter(|label| offered.iter().all(|item| item.label != *label))
        .map(|label| CompletionItem {
            sort_text: Some(format!("{}{label}", Rank::Keyword.prefix())),
            label,
            kind: Some(CompletionItemKind::KEYWORD),
            ..CompletionItem::default()
        })
        .collect::<Vec<_>>();

    // the expectations are found in no particular order
    items.sort_by(|left, right| left.label.cmp(&right.label));
    items
}
