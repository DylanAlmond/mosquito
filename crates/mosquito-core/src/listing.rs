//! Markup for the receiver's server-rendered listing (ADR-0006): the
//! top page, the per-node fragment pages, and the count-only budget
//! derivations they turn on — kept separate from the HTTP wiring in
//! [`crate::server`].
//!
//! The server derives the listing per request from the store's
//! `list_top`/`list_page` derivation, the same code path the desktop
//! paged feed consumes — equal by construction, not by mirrored
//! implementations. The top page carries loose files and one row per
//! root record and nothing beneath them; a node's page arrives only
//! through its fragment route, always materializing that node's direct
//! children server-side on the way (ADR-0005: the listing showing a
//! folder is the trigger).
//!
//! No absolute disk path and no pre-registration token reaches the
//! markup: materialized rows carry file ids and download links;
//! lazily-known rows are expandable rows without ids, never links.
//! Rendering is pure string building — the only disk access is the
//! budget rule's count-only scan, which registers nothing (display
//! never adds content, issue #24).

use std::path::Path;

use crate::file_store::{RootId, RootRecord, SharedFile};
use crate::listing_pages::{NodePage, PageRow, RootRow, TopListing};

/// How many direct children (files + subfolders) a top-level folder
/// may hold and still start expanded. ADR-0004's budget, expressed
/// once — here, on the server that stamps it. The desktop keys its
/// expansion state per node and starts every root collapsed at any
/// size (expansion there is the act that materializes, issue #24).
pub(crate) const MAX_EXPANDED_CHILDREN: usize = 5;

/// The five-children rule (ADR-0004), applied server-side with a
/// count-only scan per root record: an imported folder whose directory
/// holds at most [`MAX_EXPANDED_CHILDREN`] direct children starts
/// expanded, bigger ones collapsed. The scan registers nothing and
/// runs outside any store lock — the HTTP handler copies the root
/// records first, so the request never holds the lock across the
/// listing-wide pass (ADR-0006).
///
/// The stamps ride the same order the roots derive in —
/// [`crate::listing_pages`] walks the store's root records in
/// insertion order — so the caller zips one stamp onto each row.
pub(crate) fn default_expansions(records: &[RootRecord]) -> Vec<bool> {
    records
        .iter()
        .map(|record| count_direct_children(&record.path) <= MAX_EXPANDED_CHILDREN)
        .collect()
}

/// Count-only read of a directory's direct children, for the budget
/// rule. Symlinks are skipped — they are never shared, so they never
/// render as rows. Entries whose type can't be read are skipped too,
/// and an unreadable directory counts as empty: nothing renders from
/// it either way, and the failure surfaces inline when the node's
/// page arrives.
fn count_direct_children(path: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| match entry.file_type() {
            Ok(file_type) => !file_type.is_symlink() && (file_type.is_file() || file_type.is_dir()),
            Err(_) => false,
        })
        .count()
}

/// The page a phone sees after scanning the QR code (ADR-0006): loose
/// files lead, then one row per root record — nothing beneath the top
/// rows renders here, so the page stays bounded whatever the trees
/// hold. `root_expansions` rides the same order the roots derive in:
/// one five-children stamp per root record.
///
/// Row visuals mirror the desktop list (`FileList.vue`): file
/// checkboxes plus a select-all use the app's checkbox styling, the
/// action button where the app has "Delete selected" is the icon-only
/// download button, and depth/shade ride each row as tokens. The
/// default expansion is painted by the stamps — the enhancement
/// script seeds from them and never re-derives the budget.
pub(crate) fn render_top_listing(
    top: &TopListing,
    root_expansions: &[bool],
    device_name: &str,
) -> String {
    debug_assert_eq!(
        root_expansions.len(),
        top.roots.len(),
        "one five-children stamp per root record"
    );

    let mut items = String::new();
    for file in &top.loose {
        items.push_str(&file_row_html(file, None));
    }
    for (root, expanded_default) in top.roots.iter().zip(root_expansions) {
        items.push_str(&root_row_html(root, *expanded_default));
    }

    // Header count: loose files plus every root's import-time total.
    // Display never re-walks to re-count, so a drifted live subtree
    // may present differently than this number — the contract accepts
    // that (ADR-0006, and the count's home in `RootRow`).
    let total_files =
        top.loose.len() as u64 + top.roots.iter().map(|root| root.file_count).sum::<u64>();

    let (header, list_html) = if top.loose.is_empty() && top.roots.is_empty() {
        (
            "<p class=\"file-count\">Shared files</p>".to_string(),
            "<p class=\"empty-state\">Nothing is being shared right now.</p>".to_string(),
        )
    } else {
        (
            format!(
                "<div class=\"files-header\" role=\"row\">\
<button id=\"select-all\" type=\"button\" role=\"checkbox\" aria-checked=\"false\" aria-label=\"Select all shared files\" hidden>\
{unchecked}{checked}\
</button>\
<p class=\"file-count\">{count}</p>\
<a id=\"download-selected\" class=\"download-selected\" href=\"/download/zip\" aria-label=\"Download selected files\">\
{download_icon}\
</a>\
</div>",
                count = format!(
                    "{} file{} available",
                    total_files,
                    if total_files == 1 { "" } else { "s" }
                ),
                unchecked = UNCHECKED_BOX_SVG,
                checked = CHECKED_BOX_SVG,
                download_icon = DOWNLOAD_ICON_SVG,
            ),
            format!("<ul class=\"file-list\" role=\"rowgroup\">{items}</ul>"),
        )
    };

    include_str!("listing.html")
        .replace("{{LIST_HEADER}}", &header)
        .replace("{{DEVICE_NAME}}", &escape_html(device_name))
        .replace("{{FILE_CONTENT}}", &list_html)
}

/// One node's page as the enhancement script receives it (ADR-0006):
/// the rows wrapped in an inert `<template>` whose attributes carry
/// the page's paging state — where this window starts, how many rows
/// it held, and whether a later window exists. The script swaps the
/// rows into place after the node's own row; each row renders with the
/// depth, shade, and expansion state the server derives from the
/// node's sub path alone.
///
/// `root_key` is the node's root record display key from the same
/// `list_top` keys the top page rows carry — so one derivation keys
/// every row wherever it renders.
pub(crate) fn render_node_fragment(page: &NodePage, root_key: &str) -> String {
    let depth = node_depth(&page.sub_path);
    let region_shade = expanded_shade(depth);

    let mut rows = String::new();
    for row in &page.rows {
        match row {
            PageRow::File { file } => {
                rows.push_str(&file_row_html(file, Some((depth + 1, region_shade))));
            }
            PageRow::LazyFolder { name } => {
                let sub_path = child_sub_path(&page.sub_path, name);
                rows.push_str(&lazy_folder_row_html(
                    name,
                    page.root_id,
                    &sub_path,
                    root_key,
                    depth + 1,
                    region_shade,
                ));
            }
        }
    }
    if !page.failures.is_empty() {
        rows.push_str(&marker_row_html(
            page.failures.len(),
            depth + 1,
            region_shade,
        ));
    }

    format!(
        "<template data-node-key=\"{key}\" data-offset=\"{offset}\" \
data-window-rows=\"{window_rows}\" data-total=\"{total}\" data-has-more=\"{has_more}\">{rows}</template>",
        key = escape_html(&folder_key(root_key, &page.sub_path)),
        offset = page.offset,
        window_rows = page.rows.len(),
        total = page.total,
        has_more = if page.has_more { "true" } else { "false" },
    )
}

/// The device's own name wherever an env-provided one is available
/// (Windows `COMPUTERNAME`, elsewhere `HOSTNAME`), or a neutral fallback.
pub(crate) fn device_name() -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .into_iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .filter(|name| !name.trim().is_empty())
        })
        .unwrap_or_else(|| "Unknown device".to_string())
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;") // must be first, or we'd double-escape the rest
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Human-readable size, 1024-based (Windows Explorer style).
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    let (value, unit) = if bytes >= TB {
        (bytes as f64 / TB as f64, "TB")
    } else if bytes >= GB {
        (bytes as f64 / GB as f64, "GB")
    } else if bytes >= MB {
        (bytes as f64 / MB as f64, "MB")
    } else if bytes >= KB {
        (bytes as f64 / KB as f64, "KB")
    } else {
        return format!("{bytes} B");
    };

    format!("{value:.1} {unit}")
}

/// A node's nesting depth from its sub path: the root record's own
/// node sits at depth 0, one segment deeper each folder in the path.
/// The same reading the top page paints and each fragment's key uses.
fn node_depth(sub_path: &str) -> usize {
    sub_path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .count()
}

/// A lazily-known subfolder's own sub path: where the node's sub path
/// ends, one more segment. Never carries the absolute root path.
fn child_sub_path(node_sub_path: &str, name: &str) -> String {
    if node_sub_path.is_empty() {
        name.to_string()
    } else {
        format!("{node_sub_path}/{name}")
    }
}

/// Stable key tying a folder row to its chevron and expansion state:
/// the root's display key — from the store's derivation, where the
/// same-named `Photos` roots get the bare name first, then `Photos-2`,
/// `Photos-3`, … probing for the next free key — plus the subfolder
/// path for nested rows. Keys are appearance-ranked, so they only
/// renumber when the first-appearance order of same-named roots
/// changes. No absolute filesystem path is ever embedded: names only.
fn folder_key(root_key: &str, sub_path: &str) -> String {
    if sub_path.is_empty() {
        root_key.to_string()
    } else {
        format!("{root_key}/{sub_path}")
    }
}

/// Shading (ADR-0002/0004): a whole expanded region paints grey at
/// even depth, falls back to the plain surface at odd — alternating
/// per nesting level. Each row bakes in the shade it would show: an
/// expanded folder row carries its region's shade, its members the
/// same (one region), and a collapsed row keeps its parent's — the
/// server stamps everything once, and fragments always render under
/// already-expanded nodes, so no client-side re-shading is needed.
fn expanded_shade(depth: usize) -> u8 {
    if depth % 2 == 0 { 1 } else { 0 }
}

/// One root record's row: the tree's top, lazily-known and collapsed
/// unless the five-children stamp says otherwise. The row never
/// renders children (its page arrives on expansion), no checkbox
/// subtree payload (selection resolves through the node, ADR-0006;
/// node-level resolution rides the archive ticket — so lazily-known
/// rows carry no member ids on the receiver page), and no absolute
/// path or pre-registration token: the node's address for its
/// fragment rides the root record id plus the (relative) sub path.
fn root_row_html(root: &RootRow, expanded_default: bool) -> String {
    let key = escape_html(&root.key);
    let name = escape_html(&root.name);
    let depth = 0;
    let shade = if expanded_default {
        expanded_shade(depth)
    } else {
        0
    };

    format!(
        "<li class=\"folder-row\" role=\"row\" data-depth=\"{depth}\" data-folder-key=\"{key}\" \
data-node-root=\"{id}\" data-node-sub=\"\" data-shade=\"{shade}\" style=\"--depth:{depth}\">\
<span class=\"checkbox-spacer\" aria-hidden=\"true\"></span>\
<button type=\"button\" class=\"folder-chevron{collapsed}\" aria-expanded=\"{expanded}\" \
aria-label=\"{chevron_label} {name}\" data-toggle=\"{key}\" hidden>{chevron_svg}</button>\
<span class=\"folder-name\">{name}</span>\
<small class=\"folder-count\">{count}</small></li>",
        id = root.id,
        collapsed = if expanded_default {
            ""
        } else {
            " is-collapsed"
        },
        expanded = if expanded_default { "true" } else { "false" },
        chevron_label = if expanded_default {
            "Collapse"
        } else {
            "Expand"
        },
        chevron_svg = CHEVRON_SVG,
        count = format!(
            "{} {}",
            root.file_count,
            if root.file_count == 1 {
                "file"
            } else {
                "files"
            }
        ),
    )
}

/// One lazily-known subfolder row, as a node's fragment renders it:
/// expandable only — no id, no link, no count, no checkbox payload.
/// Its own node address rides the root record id plus its sub path
/// for when the client fetches *its* page next.
fn lazy_folder_row_html(
    name: &str,
    root_id: RootId,
    sub_path: &str,
    root_key: &str,
    depth: usize,
    shade: u8,
) -> String {
    let key = escape_html(&folder_key(root_key, sub_path));
    let name = escape_html(name);

    format!(
        "<li class=\"folder-row\" role=\"row\" data-depth=\"{depth}\" data-folder-key=\"{key}\" \
data-node-root=\"{root_id}\" data-node-sub=\"{sub_path}\" data-shade=\"{shade}\" \
style=\"--depth:{depth}\">\
<span class=\"checkbox-spacer\" aria-hidden=\"true\"></span>\
<button type=\"button\" class=\"folder-chevron is-collapsed\" aria-expanded=\"false\" \
aria-label=\"Expand {name}\" data-toggle=\"{key}\" hidden>{chevron_svg}</button>\
<span class=\"folder-name\">{name}</span></li>",
        root_id = root_id,
        sub_path = escape_html(sub_path),
        chevron_svg = CHEVRON_SVG,
    )
}

/// One materialization-failure marker row (ADR-0005): the count of
/// this folder's unreadable members, inline and never blocking the
/// rest. Unlike the desktop's marker there is no tooltip listing the
/// failing paths — absolute paths never reach receiver markup.
fn marker_row_html(count: usize, depth: usize, shade: u8) -> String {
    format!(
        "<li class=\"materialize-marker\" role=\"row\" data-depth=\"{depth}\" data-shade=\"{shade}\" \
style=\"--depth:{depth}\">\
<span class=\"checkbox-spacer\" aria-hidden=\"true\"></span>\
<p class=\"marker-text\">{text}</p></li>",
        text = format!(
            "{} item{} of this folder couldn't be read",
            count,
            if count == 1 { "" } else { "s" }
        ),
    )
}

/// One selectable file row: a plain download link (works without
/// JavaScript) wrapped around the name and size, with the selection
/// checkbox beside it — revealed only when JavaScript runs. Tree
/// members pass `(depth, shade)`; loose files sit outside any tree
/// and carry none of that.
fn file_row_html(file: &SharedFile, tree_row: Option<(usize, u8)>) -> String {
    let title = file.name.as_str();

    let attrs = match tree_row {
        Some((depth, shade)) => {
            format!(" data-depth=\"{depth}\" data-shade=\"{shade}\" style=\"--depth:{depth}\"")
        }
        None => String::new(),
    };

    format!(
        "<li class=\"file-row\" role=\"row\"{attrs}>\
<button type=\"button\" role=\"checkbox\" aria-checked=\"false\" aria-label=\"Select {title}\" data-select=\"{id}\" hidden>\
{unchecked_svg}{checked_svg}\
</button>\
<a href=\"/download/{id}\"><span>{title}</span><small>{size}</small></a>\
</li>",
        id = file.id.0,
        title = escape_html(title),
        size = format_size(file.size),
        unchecked_svg = UNCHECKED_BOX_SVG,
        checked_svg = CHECKED_BOX_SVG,
    )
}

/// The app's checkbox artwork (desktop `square-rounded.svg`), inlined so
/// the listing stays a single self-contained page.
const UNCHECKED_BOX_SVG: &str = "<svg class=\"icon-unchecked\" xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path stroke=\"none\" d=\"M0 0h24v24H0z\" fill=\"none\" /><path d=\"M12 3c7.2 0 9 1.8 9 9c0 7.2 -1.8 9 -9 9c-7.2 0 -9 -1.8 -9 -9c0 -7.2 1.8 -9 9 -9\" /></svg>";

const CHECKED_BOX_SVG: &str = "<svg class=\"icon-checked\" xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path stroke=\"none\" d=\"M0 0h24v24H0z\" fill=\"none\" /><path d=\"M9 12l2 2l4 -4\" /><path d=\"M12 3c7.2 0 9 1.8 9 9c0 7.2 -1.8 9 -9 9c-7.2 0 -9 -1.8 -9 -9c0 -7.2 1.8 -9 9 -9\" /></svg>";

/// The project's download icon, for the
/// icon-only download button where the app has "Delete selected". The
/// desktop list inlines its own copy in `FileList.vue` (the deleted
/// shared asset went unused); visual language is mirrored by duplicated
/// tokens, not shared assets — ADR-0003.
const DOWNLOAD_ICON_SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path stroke=\"none\" d=\"M0 0h24v24H0z\" fill=\"none\" /><path d=\"M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2 -2v-2\" /><path d=\"M7 11l5 5l5 -5\" /><path d=\"M12 4l0 12\" /></svg>";

/// The desktop's chevron artwork (`chevron-compact-up.svg`) for the
/// expand/collapse control: points up when expanded, the client
/// rotates it when the folder is collapsed.
const CHEVRON_SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path stroke=\"none\" d=\"M0 0h24v24H0z\" fill=\"none\" /><path d=\"M4 13l8 -3l8 3\" /></svg>";

#[cfg(test)]
mod tests {
    use super::*;

    use crate::file_store::{FailedAdd, FileId};
    use crate::listing_pages::LISTING_PAGE_ROWS;
    use crate::test_support::{member, nowhere, shared_file};

    // --- escape_html ---

    #[test]
    fn escape_html_escapes_all_specials() {
        assert_eq!(escape_html(r#"&<>"'"#), "&amp;&lt;&gt;&quot;&#39;");
    }

    #[test]
    fn escape_html_ampersand_first_never_double_escapes() {
        assert_eq!(escape_html("&lt;"), "&amp;lt;");
    }

    // --- format_size ---

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
    }

    #[test]
    fn format_size_kilobytes() {
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
    }

    #[test]
    fn format_size_megabytes_and_gigabytes() {
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    // --- render helpers for the tests: hand-made root rows and node
    // pages, with fabricated paths — every render then doubles as a
    // no-path-leak check.

    fn root_row(id: u64, name: &str, key: &str, file_count: u64) -> RootRow {
        RootRow {
            id: RootId(id),
            name: name.into(),
            key: key.into(),
            file_count,
        }
    }

    fn file_page_row(id: u64, name: &str) -> PageRow {
        PageRow::File {
            file: shared_file(id, name, None),
        }
    }

    fn node_page(
        root_id: u64,
        sub_path: &str,
        offset: usize,
        rows: Vec<PageRow>,
        failures: Vec<FailedAdd>,
        total: usize,
        has_more: bool,
    ) -> NodePage {
        NodePage {
            root_id: RootId(root_id),
            sub_path: sub_path.into(),
            offset,
            rows,
            failures,
            total,
            has_more,
        }
    }

    // --- render_top_listing ---

    #[test]
    fn listing_shows_empty_message_without_links() {
        let top = TopListing {
            loose: Vec::new(),
            roots: Vec::new(),
        };
        let html = render_top_listing(&top, &[], "test-device");

        assert!(html.contains("Nothing is being shared"));
        assert!(!html.contains("<li>"));
        assert!(html.contains("Shared from test-device"));
        assert!(html.contains("https://github.com/DylanAlmond/mosquito"));
        assert!(!html.contains("{{"));
    }

    #[test]
    fn listing_loose_files_come_before_the_root_rows() {
        let top = TopListing {
            loose: vec![shared_file(1, "a.txt", None)],
            roots: vec![
                root_row(7, "Photos", "Photos", 2),
                root_row(8, "Backup", "Backup", 5),
            ],
        };

        let html = render_top_listing(&top, &[false, false], "test-device");

        // Loose files lead, ungrouped; one row per root record after.
        let loose_start = html.find("href=\"/download/1\"").unwrap();
        let first_root = html.find("data-folder-key=\"Photos\"").unwrap();
        let second_root = html.find("data-folder-key=\"Backup\"").unwrap();
        assert!(loose_start < first_root, "loose files first");
        assert!(first_root < second_root, "roots in insertion order");
    }

    #[test]
    fn listing_renders_one_row_per_root_record_with_count() {
        // A root-record-only import: name and import-time count render,
        // no members, no links, no subtree payload anywhere.
        let top = TopListing {
            loose: Vec::new(),
            roots: vec![root_row(3, "Photographs", "Photographs", 2)],
        };

        let html = render_top_listing(&top, &[false], "test-device");

        assert_eq!(html.matches("class=\"folder-row\"").count(), 1);
        assert!(html.contains("class=\"folder-count\">2 files</small>"));
        assert!(html.contains("aria-expanded=\"false\""));
        // Nothing beneath the top rows: no member file rows, hence no
        // file checkboxes either — the only /download href on the page
        // is the header's own zip button.
        assert!(!html.contains("class=\"file-row\""));
        assert!(!html.contains("data-select="));
        // No subtree checkbox payload: selection resolves through the
        // node, and its node-level form rides the archive ticket.
        assert!(!html.contains("data-subtree"));
        // The node address rides the record id, never a path.
        assert!(html.contains("data-node-root=\"3\" data-node-sub=\"\""));
    }

    #[test]
    fn listing_stamps_five_children_rule_from_the_expansion_vector() {
        let top = TopListing {
            loose: Vec::new(),
            roots: vec![
                root_row(0, "Small", "Small", 1),
                root_row(1, "Big", "Big", 9),
            ],
        };

        let html = render_top_listing(&top, &[true, false], "test-device");

        // A within-budget root starts expanded (server stamp): the grey
        // region paints on the row and the chevron reads open.
        assert!(html.contains(
            "data-folder-key=\"Small\" data-node-root=\"0\" data-node-sub=\"\" data-shade=\"1\""
        ));
        assert!(html.contains("aria-label=\"Collapse Small\""));
        // A bigger root starts collapsed: parent shade, folded chevron.
        assert!(html.contains(
            "data-folder-key=\"Big\" data-node-root=\"1\" data-node-sub=\"\" data-shade=\"0\""
        ));
        assert!(html.contains("aria-label=\"Expand Big\""));
        assert!(html.contains("class=\"folder-chevron is-collapsed\""));
    }

    #[test]
    fn listing_counts_header_files_from_loose_and_import_totals() {
        let top = TopListing {
            loose: vec![shared_file(0, "a.txt", None)],
            roots: vec![
                root_row(0, "Photos", "Photos", 2),
                root_row(1, "Empty", "Empty", 0),
            ],
        };

        let html = render_top_listing(&top, &[false, false], "test-device");

        assert!(html.contains("3 files available"));
    }

    #[test]
    fn listing_header_counts_a_single_file_singularity() {
        let top = TopListing {
            loose: vec![shared_file(0, "a.txt", None)],
            roots: Vec::new(),
        };

        let html = render_top_listing(&top, &[], "test-device");

        assert!(html.contains("1 file available"));
    }

    #[test]
    fn listing_keys_root_rows_with_the_derivation_s_suffixes() {
        // The suffixing itself is the store's derivation (tested in
        // `listing_pages`); the top page keys rows by the derived
        // keys, never by render position.
        let top = TopListing {
            loose: Vec::new(),
            roots: vec![
                root_row(0, "Photos", "Photos", 1),
                root_row(1, "Photos", "Photos-2", 1),
            ],
        };

        let html = render_top_listing(&top, &[false, false], "test-device");

        assert_eq!(html.matches("data-folder-key=\"Photos\"").count(), 1);
        assert_eq!(html.matches("data-folder-key=\"Photos-2\"").count(), 1);
        assert!(!html.contains("data-folder-key=\"0\""), "no render index");
    }

    #[test]
    fn listing_loose_files_carry_no_tree_markup() {
        let top = TopListing {
            loose: vec![shared_file(0, "a.txt", None)],
            roots: Vec::new(),
        };

        let html = render_top_listing(&top, &[], "test-device");

        let row_start = html.find("<li class=\"file-row\"").unwrap();
        let row_end = html[row_start..].find("</li>").unwrap() + row_start;
        let row = &html[row_start..row_end];
        assert!(!row.contains("data-depth"));
        assert!(!row.contains("data-shade"));
        assert!(!row.contains("data-folder-key"));
        assert!(!row.contains("style="));
    }

    #[test]
    fn listing_never_leaks_absolute_paths_into_the_markup() {
        // Root names derive from display names only: no test path
        // (absolute temp paths through `nowhere()`) may appear — not
        // on the top page, and never in an attribute stamp.
        let top = TopListing {
            loose: vec![
                shared_file(0, "loose.txt", None),
                member(1, "beach.jpg", "Photos", "beach.jpg"),
                member(2, "trip.txt", "Backup/Photos", "notes/trip.txt"),
            ],
            roots: vec![root_row(4, "Photos", "Photos", 12)],
        };

        let html = render_top_listing(&top, &[false], "test-device");

        assert!(!html.contains("mosquito-nowhere"));
        assert!(!html.contains(nowhere("Photos").to_string_lossy().as_ref()));
        assert!(!html.contains(nowhere("Backup/Photos").to_string_lossy().as_ref()));
        assert!(!html.contains(std::env::temp_dir().to_string_lossy().as_ref()));
    }

    #[test]
    fn listing_renders_desktop_style_selection_ui() {
        let top = TopListing {
            loose: vec![shared_file(0, "a.txt", None)],
            roots: Vec::new(),
        };

        let html = render_top_listing(&top, &[], "test-device");

        // Select-all checkbox in the header, per-file checkboxes in
        // rows, using the app's own square-rounded checkbox artwork.
        assert!(html.contains("aria-label=\"Select all shared files\""));
        assert!(html.contains("data-select=\"0\" hidden"));
        assert!(html.contains("icon-checked"));
        // The download button starts hidden: selection is empty and
        // the inline script only reveals it once something is selected.
        assert!(html.contains("id=\"download-selected\" class=\"download-selected\""));
        assert!(!html.contains("download-selected is-visible"));
        // Selection is wired by an inline vanilla script: no framework
        // and no build step anywhere in the page.
        assert!(html.contains("<script>"));
        assert!(html.contains("addEventListener"));
        // Plain per-file download links remain, so downloads work with
        // JavaScript disabled.
        assert!(html.contains("href=\"/download/0\""));
        // The selection script never lists subtree member ids —
        // lazily-known rows resolve selection through the node
        // (node-level resolution rides the archive ticket).
        assert!(!html.contains("data-subtree"));
    }

    #[test]
    fn listing_escapes_names_and_rows_use_plain_download_links() {
        let top = TopListing {
            loose: vec![SharedFile {
                id: FileId(0),
                name: "<script>.txt".into(),
                path: "unused".into(),
                size: 42,
                provenance: None,
            }],
            roots: Vec::new(),
        };

        let html = render_top_listing(&top, &[], "test-device");

        assert!(html.contains("&lt;script&gt;.txt"));
        assert!(html.contains("href=\"/download/0\""));
        assert!(html.contains("42 B"));
        // Narrowed to the raw name: the page legitimately contains
        // its own enhancement <script>, so a whole-document check
        // would false-positive on it.
        assert!(!html.contains("<script>.txt"));
        assert!(
            html.find("class=\"file-count\"").unwrap() < html.find("class=\"file-list\"").unwrap()
        );
        assert!(html.contains("overflow-y: auto"));
    }

    #[test]
    fn listing_escapes_the_device_name() {
        let top = TopListing {
            loose: Vec::new(),
            roots: Vec::new(),
        };
        let html = render_top_listing(&top, &[], "<script>host</script>");

        assert!(html.contains("Shared from &lt;script&gt;host&lt;/script&gt;"));
        assert!(!html.contains("<script>host</script>"));
    }

    // --- default_expansions ---

    #[test]
    fn budget_stamps_scan_only_the_node_s_direct_children() {
        use crate::test_support::disk::make_file;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let root = dir.path().join("Photos");
        std::fs::create_dir_all(root.join("vacation").join("deeper")).unwrap();
        make_file(&root, "a.txt", b"a");
        make_file(&root, "b.txt", b"b");
        make_file(&root.join("vacation"), "trip.jpg", b"t");

        let record = RootRecord {
            id: RootId(0),
            path: root.clone(),
            file_count: 3,
        };
        let expansions = default_expansions(&[record]);

        // Two files + one directory at this level: the deeper levels
        // don't walk — `trip.jpg` and `deeper` live under `vacation` —
        // and the budget reads them all the same either way.
        let count = count_direct_children(&root);
        assert_eq!(count, 3);
        assert_eq!(expansions, vec![count <= MAX_EXPANDED_CHILDREN]);
    }

    #[test]
    fn budget_collapses_roots_over_the_five_children_line() {
        use crate::test_support::disk::make_file;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let root = dir.path().join("Big");
        std::fs::create_dir(&root).unwrap();
        for name in ["aaa", "bbb", "ccc", "ddd", "eee"] {
            make_file(&root, name, b"x");
        }
        std::fs::create_dir(root.join("fff")).unwrap();

        // Six direct children exactly: over the budget, starts
        // collapsed — the boundary the five-children rule pins.
        let records = vec![RootRecord {
            id: RootId(0),
            path: root,
            file_count: 5,
        }];
        let expansions = default_expansions(&records);
        assert_eq!(expansions, vec![false]);
    }

    #[test]
    fn budget_counts_an_unreadable_root_as_empty() {
        // A missing directory scans as zero children: the stamps only
        // feed the fold decision, and the node's failure surfaces
        // inline when its page is asked for.
        let records = vec![RootRecord {
            id: RootId(0),
            path: nowhere("Photos/moved-on"),
            file_count: 99,
        }];

        let expansions = default_expansions(&records);
        assert_eq!(expansions, vec![true]);
    }

    #[test]
    fn budget_skips_symlinks_like_the_materialization_does() {
        use crate::test_support::disk::make_file;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir(&root).unwrap();
        make_file(&root, "plain.txt", b"p");

        let target = make_file(dir.path(), "outside.txt", b"top secret");
        #[cfg(windows)]
        let link = match std::os::windows::fs::symlink_file(&target, root.join("link.txt")) {
            Ok(()) => root.join("link.txt"),
            Err(_) => return, // can't create symlinks in this environment
        };
        #[cfg(unix)]
        let link = match std::os::unix::fs::symlink(&target, root.join("link.txt")) {
            Ok(()) => root.join("link.txt"),
            Err(_) => return,
        };
        #[cfg(not(any(windows, unix)))]
        let _link_unused = ();

        // One regular file: the link never counts (it is never
        // shared, so it would never render as a row).
        let count = count_direct_children(&root);
        assert_eq!(count, 1);
        let _ = link;
    }

    // --- render_node_fragment ---

    #[test]
    fn fragment_lists_the_pages_rows_with_depth_and_shade() {
        let page = node_page(
            7,
            "",
            0,
            vec![
                file_page_row(4, "beach.jpg"),
                PageRow::LazyFolder {
                    name: "vacation".into(),
                },
            ],
            Vec::new(),
            2,
            false,
        );

        let html = render_node_fragment(&page, "Photos");

        // The paging state rides the template's own attributes.
        assert!(html.contains("<template data-node-key=\"Photos\""));
        assert!(html.contains("data-offset=\"0\""));
        assert!(html.contains("data-window-rows=\"2\""));
        assert!(html.contains("data-total=\"2\""));
        assert!(html.contains("data-has-more=\"false\""));

        // The root's own rows: files at depth 1 inside the depth-0
        // grey region, the lazily-known folder beside them.
        assert!(html.contains("href=\"/download/4\""));
        assert!(html.contains("data-depth=\"1\" data-shade=\"1\" style=\"--depth:1\""));
        assert!(html.contains(
            "data-folder-key=\"Photos/vacation\" data-node-root=\"7\" data-node-sub=\"vacation\""
        ));
        assert!(html.contains("aria-expanded=\"false\""));
        // Lazily-known rows are expandable only: no ids, no links.
        let lazy_start = html.find("data-folder-key=\"Photos/vacation\"").unwrap();
        let lazy_end = lazy_start + html[lazy_start..].find("</li>").unwrap();
        let lazy = &html[lazy_start..lazy_end];
        assert!(!lazy.contains("href="));
        assert!(!lazy.contains("data-select"));
    }

    #[test]
    fn fragment_rides_the_sub_paths_depth_shading_and_addressing() {
        let page = node_page(
            2,
            "vacation",
            0,
            vec![
                file_page_row(5, "trip.jpg"),
                PageRow::LazyFolder {
                    name: "deep".into(),
                },
            ],
            Vec::new(),
            2,
            false,
        );

        let html = render_node_fragment(&page, "Photos");

        // One segment down: the node sits at depth 1 (odd), so its
        // region is the plain surface and its rows carry depth 2.
        assert!(html.contains("data-node-key=\"Photos/vacation\""));
        assert!(html.contains("data-depth=\"2\" data-shade=\"0\" style=\"--depth:2\""));
        // The nested lazy row's own sub path composes on the node's.
        assert!(html.contains("data-node-root=\"2\" data-node-sub=\"vacation/deep\""));
        assert!(html.contains("data-folder-key=\"Photos/vacation/deep\""));
        // The markup carries the relative sub path, never the root's
        // absolute one.
        assert!(!html.contains("mosquito-nowhere"));
    }

    #[test]
    fn fragment_reports_unreadable_members_as_an_inline_marker() {
        let page = node_page(
            0,
            "sealed",
            0,
            Vec::new(),
            vec![FailedAdd {
                path: nowhere("Photos/sealed"),
                error: "access denied".into(),
            }],
            0,
            false,
        );

        let html = render_node_fragment(&page, "Photos");

        assert!(html.contains("1 item of this folder couldn't be read"));
        // The marker renders the count, never the failing paths:
        // absolute disk paths never reach receiver markup.
        assert!(!html.contains("mosquito-nowhere"));
    }

    #[test]
    fn fragment_marker_pluralizes() {
        let page = node_page(
            0,
            "",
            0,
            Vec::new(),
            vec![
                FailedAdd {
                    path: nowhere("Photos/a"),
                    error: "e".into(),
                },
                FailedAdd {
                    path: nowhere("Photos/b"),
                    error: "e".into(),
                },
            ],
            0,
            false,
        );

        let html = render_node_fragment(&page, "Photos");

        assert!(html.contains("2 items of this folder couldn't be read"));
    }

    #[test]
    fn fragment_stamps_continuation_windows() {
        // A mid-node window: the offsets and the has-more stamp tell
        // the client where a later fetch continues.
        let page = node_page(
            0,
            "",
            LISTING_PAGE_ROWS,
            vec![PageRow::LazyFolder {
                name: "tail".into(),
            }],
            Vec::new(),
            LISTING_PAGE_ROWS + 1,
            false,
        );

        let html = render_node_fragment(&page, "Photos");

        assert!(html.contains("data-offset=\"200\""));
        assert!(html.contains("data-window-rows=\"1\""));
        assert!(html.contains("data-total=\"201\""));
        assert!(html.contains("data-has-more=\"false\""));
    }

    #[test]
    fn fragment_keys_compose_from_the_root_key_and_sub_paths() {
        let page = node_page(
            0,
            "vacation",
            0,
            vec![PageRow::LazyFolder {
                name: "deep".into(),
            }],
            Vec::new(),
            1,
            false,
        );

        // A second root record with the same display name keeps its
        // disambiguated key from the derivation — fragments key off it,
        // so both roots' nested rows stay distinct.
        let html = render_node_fragment(&page, "Photos-2");

        assert!(html.contains("data-node-key=\"Photos-2/vacation\""));
        assert!(html.contains("data-folder-key=\"Photos-2/vacation/deep\""));
    }

    #[test]
    fn fragment_escapes_names_keys_and_sub_paths() {
        let page = node_page(
            0,
            "<script>",
            0,
            vec![PageRow::LazyFolder {
                name: "<b>folder</b>".into(),
            }],
            Vec::new(),
            1,
            false,
        );

        let html = render_node_fragment(&page, "Photos");

        assert!(html.contains("data-node-key=\"Photos/&lt;script&gt;\""));
        assert!(html.contains("<span class=\"folder-name\">&lt;b&gt;folder&lt;/b&gt;</span>"));
        assert!(!html.contains("<b>folder</b></span>"));
        // A raw name must never feed an attribute: escaped everywhere.
        assert!(html.contains(
            "data-node-root=\"0\" data-node-sub=\"&lt;script&gt;/&lt;b&gt;folder&lt;/b&gt;\""
        ));
        // The chevron label escapes too.
        assert!(!html.contains("aria-label=\"Expand <b>folder"));
    }
}
