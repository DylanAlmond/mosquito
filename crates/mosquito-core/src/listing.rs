//! Renders `GET /`'s page markup: the tree markup helpers plus the
//! inline SVG artwork, kept separate from the HTTP wiring in
//! [`crate::server`].
//!
//! Pure string building — no store, no I/O — so the page rules are
//! unit-testable from the shared-file snapshots alone.

use std::collections::HashSet;

use crate::file_store::SharedFile;
use crate::folder_tree::{FolderNode, build_folder_tree};

/// The page a phone sees after scanning the QR code.
///
/// Rendering mirrors the desktop list (`FileList.vue`): file checkboxes
/// plus a select-all use the app's checkbox styling, folders render as
/// expandable tree rows (the Rust mirror of `folderTree.ts`, ADR-0002),
/// and the action button where the app has "Delete selected" is the
/// icon-only download button.
///
/// The server emits the default expansion: folder roots expanded,
/// subfolders collapsed (their members rendered `hidden` so the
/// enhancement script can reveal them without any dynamic markup). The
/// script assumes JavaScript is present, like the checkboxes already do.
pub(crate) fn render_listing(files: &[SharedFile], device_name: &str) -> String {
    // Folder tree derivation at render time (ADR-0002): loose files
    // lead, then one root node per distinct folder root in
    // first-appearance order — the desktop list's exact layout.
    let tree = build_folder_tree(files);

    let mut items = String::new();
    for file in &tree.loose {
        items.push_str(&file_row_html(file, None));
    }
    let keys = root_keys(&tree.roots);
    for (root, root_key) in tree.roots.iter().zip(&keys) {
        render_folder(root, root_key, &mut items, true, 0);
    }

    let (header, list_html) = if files.is_empty() {
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
                    files.len(),
                    if files.len() == 1 { "" } else { "s" }
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

/// Stable key tying a folder row to its chevron and expansion state:
/// the folder root's display name — disambiguated only when two folder
/// roots share a name (the second appearance of `Photos` becomes
/// `Photos-2`, the third `Photos-3`, …), mirroring the archive
/// planner's collision handling — plus the subfolder path for nested
/// rows. The name stands in for the folder root's identity (the scheme
/// the desktop list keys its state by); unlike on the desktop, the key
/// is appearance-ranked rather than path-based, so a key is only
/// renumbered when the first-appearance order of same-named roots
/// changes — e.g. when the earlier same-named root is removed. No
/// absolute filesystem path is ever embedded: the name only.
fn folder_key(root_key: &str, sub_path: &str) -> String {
    if sub_path.is_empty() {
        root_key.to_string()
    } else {
        format!("{root_key}/{sub_path}")
    }
}

/// One root key per root, in first-appearance order: the root's display
/// name, kept bare for the first root to carry it and suffixed for each
/// later same-named one. Probing the next free suffix — like the
/// archive planner's `unique_entry_path` — keeps the keys distinct even
/// when a root is literally named `Photos-2` already. Deterministic:
/// the same shared-file list always yields the same key assignment.
fn root_keys(roots: &[FolderNode<'_>]) -> Vec<String> {
    let mut keys = Vec::with_capacity(roots.len());
    let mut used: HashSet<String> = HashSet::new();
    for root in roots {
        let mut ordinal = 2;
        let mut candidate = root.name.clone();
        while !used.insert(candidate.clone()) {
            candidate = format!("{}-{ordinal}", root.name);
            ordinal += 1;
        }
        keys.push(candidate);
    }
    keys
}

/// Shading (ADR-0002): a whole expanded region paints grey at even
/// depth, falls back to the plain surface at odd — alternating per
/// nesting level. The depth geometry of ADR-0004 rides each tree row
/// as its `--depth` token (name-text indent, chevron carry, member
/// catch-up — the stylesheet and the client-side script turn the token
/// into pixels; loose rows carry no token). Shading stays computed,
/// since a collapsed folder row keeps its parent's shade. The server
/// paints the default expansion; the client recomputes on every toggle.
fn expanded_shade(depth: usize) -> u8 {
    if depth % 2 == 0 { 1 } else { 0 }
}

/// All file ids under `node` — its own members plus every nested
/// subfolder's, collapsed members included, so a folder checkbox acts
/// on the full subtree whatever is visible.
fn subtree_ids(node: &FolderNode<'_>, out: &mut Vec<u64>) {
    out.extend(node.files.iter().map(|file| file.id.0));
    for child in &node.children {
        subtree_ids(child, out);
    }
}

fn subtree_ids_string(node: &FolderNode<'_>) -> String {
    let mut ids = Vec::new();
    subtree_ids(node, &mut ids);
    ids.iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// What the default expansion makes of the region: a root is visible
/// with its contents shown; a subfolder row shows while its members
/// (and nested rows) render hidden for the enhancement script to
/// reveal. `parent_shade` is the region shade the row fell out of — a
/// collapsed folder row keeps its parent's background like the desktop
/// does.
fn render_folder(
    node: &FolderNode<'_>,
    root_key: &str,
    items: &mut String,
    visible: bool,
    parent_shade: u8,
) {
    let key = escape_html(&folder_key(root_key, &node.sub_path));
    let expanded_region = expanded_shade(node.depth);
    let expanded_default = node.depth == 0;
    let row_shade = if expanded_default {
        expanded_region
    } else {
        parent_shade
    };

    let name = escape_html(&node.name);
    items.push_str(&format!(
        "<li class=\"folder-row\" role=\"row\"{hidden} data-depth=\"{depth}\" data-folder-key=\"{key}\" data-shade=\"{shade}\" style=\"--depth:{depth}\">\
<button type=\"button\" role=\"checkbox\" aria-checked=\"false\" aria-label=\"Select all files in {name}\" data-subtree=\"{subtree}\" hidden>\
{unchecked_svg}{checked_svg}\
</button>\
<button type=\"button\" class=\"folder-chevron\" aria-expanded=\"{expanded}\" aria-label=\"{chevron_label} {name}\" data-toggle=\"{key}\" hidden>\
{chevron_svg}\
</button>\
<span class=\"folder-name\">{name}</span></li>",
        hidden = if visible { "" } else { " hidden" },
        depth = node.depth,
        shade = row_shade,
        subtree = subtree_ids_string(node),
        expanded = if expanded_default { "true" } else { "false" },
        chevron_label = if expanded_default { "Collapse" } else { "Expand" },
        chevron_svg = CHEVRON_SVG,
        unchecked_svg = UNCHECKED_BOX_SVG,
        checked_svg = CHECKED_BOX_SVG,
    ));

    // Rows show the file's name; where it hangs under the root is the
    // tree's job (desktop `memberTitle()`). Content rows take the
    // region shade of the folder that would show them; the client
    // re-shades whatever it reveals. Subfolders are collapsed by
    // default, so everything beneath a subfolder renders hidden.
    let child_visible = visible && expanded_default;
    for file in &node.files {
        items.push_str(&file_row_html(
            file,
            Some(&RowContext {
                depth: node.depth + 1,
                shade: expanded_region,
                visible: child_visible,
            }),
        ));
    }
    for child in &node.children {
        render_folder(child, root_key, items, child_visible, row_shade);
    }
}

/// Region context a tree member row renders with: its depth, the shade
/// of the region that would show it, and whether the default expansion
/// shows it.
struct RowContext {
    depth: usize,
    shade: u8,
    visible: bool,
}

/// One selectable file row: a plain download link (works without
/// JavaScript) wrapped around the name and size, with the selection
/// checkbox beside it — revealed only when JavaScript runs. Tree
/// members pass a [`RowContext`]; loose files sit outside any tree and
/// carry none of that.
fn file_row_html(file: &SharedFile, tree_row: Option<&RowContext>) -> String {
    let title = file.name.as_str();

    let attrs = match tree_row {
        Some(context) => format!(
            " data-depth=\"{depth}\" data-shade=\"{shade}\"{hidden} style=\"--depth:{depth}\"",
            depth = context.depth,
            shade = context.shade,
            hidden = if context.visible { "" } else { " hidden" },
        ),
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

    use crate::file_store::FileId;
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

    // --- render_listing ---

    #[test]
    fn listing_shows_empty_message_without_links() {
        let html = render_listing(&[], "test-device");

        assert!(html.contains("Nothing is being shared"));
        assert!(!html.contains("<li>"));
        assert!(html.contains("Shared from test-device"));
        assert!(html.contains("https://github.com/DylanAlmond/mosquito"));
        assert!(!html.contains("{{"));
    }

    #[test]
    fn listing_loose_files_come_before_folder_trees() {
        let files = vec![
            member(0, "b.txt", "Photos", "sub/b.txt"),
            shared_file(1, "a.txt", None),
            member(2, "a.txt", "Photos", "a.txt"),
        ];

        let html = render_listing(&files, "test-device");

        // Loose files lead, ungrouped; the folder tree follows at the
        // folder root's first member's position (first-appearance order).
        let loose = html.find("href=\"/download/1\"").unwrap();
        let folder = html.find("class=\"folder-row\"").unwrap();
        let member_row = html.find("href=\"/download/0\"").unwrap();
        assert!(loose < folder, "loose files first");
        assert!(folder < member_row, "folder members under their folder row");
    }

    #[test]
    fn listing_renders_roots_expanded_and_subfolders_collapsed_by_default() {
        // Photographs holds a direct member and a nested one; the tree
        // is Photographs (expanded) > beach.jpg + notes (collapsed, its
        // members rendered but hidden for the enhancement script).
        let files = vec![
            shared_file(0, "recipes.txt", None),
            member(1, "beach.jpg", "Photographs", "beach.jpg"),
            member(2, "todo.txt", "Photographs", "notes/todo.txt"),
        ];

        let html = render_listing(&files, "test-device");

        // The root row is an expandable folder row, expanded by default.
        assert!(html.contains("class=\"folder-row\""));
        assert!(html.contains("aria-expanded=\"true\""));
        assert!(html.contains("aria-label=\"Collapse Photographs\""));

        // The `notes` subfolder row itself is visible but collapsed, and
        // its members are rendered hidden for the enhancement script to
        // reveal. A collapsed folder row keeps its parent's background
        // (story 10) — here the grey of the expanded root's region.
        assert!(html.contains("aria-expanded=\"false\""));
        assert!(html.contains("aria-label=\"Expand notes\""));
        assert!(html.contains("data-folder-key=\"Photographs/notes\" data-shade=\"1\""));
        let collapsed_member = html
            .find("<li class=\"file-row\" role=\"row\" data-depth=\"2\" data-shade=\"0\" hidden")
            .expect("collapsed members render hidden");
        let notes_row = html
            .find("<span class=\"folder-name\">notes</span>")
            .unwrap();
        assert!(notes_row < collapsed_member);

        // Root content sits at depth 1 under the depth-0 root.
        assert!(html.contains("data-depth=\"0\""));
        assert!(html.contains("data-depth=\"1\""));
    }

    #[test]
    fn listing_loose_files_carry_no_tree_markup() {
        let files = vec![shared_file(0, "a.txt", None)];

        let html = render_listing(&files, "test-device");

        let row_start = html.find("<li class=\"file-row\"").unwrap();
        let row_end = html[row_start..].find("</li>").unwrap() + row_start;
        let row = &html[row_start..row_end];
        assert!(!row.contains("data-depth"));
        assert!(!row.contains("data-shade"));
        assert!(!row.contains("data-folder-key"));
        assert!(!row.contains("style="));
    }

    #[test]
    fn listing_stamps_tree_members_with_their_depth_token() {
        // The depth each member renders at — root 0, direct member 1,
        // member under the subfolder 2 — rides each tree row as its
        // `--depth` custom property (ADR-0004). The stylesheet and the
        // client script turn the token into the name-text indent, the
        // chevron carry, and the member catch-up; loose rows carry no
        // token (asserted in listing_loose_files_carry_no_tree_markup).
        let files = vec![
            shared_file(0, "top.txt", None),
            member(1, "beach.jpg", "Photographs", "beach.jpg"),
            member(2, "todo.txt", "Photographs", "notes/todo.txt"),
        ];

        let html = render_listing(&files, "test-device");

        // Root row (depth 0, its expanded region is grey).
        assert!(html.contains(r#"data-folder-key="Photographs" data-shade="1" style="--depth:0""#));
        // Direct member: depth 1, visible inside the root's grey region.
        assert!(html.contains(r#"data-depth="1" data-shade="1" style="--depth:1""#));
        // Member under the collapsed `notes` subfolder: depth 2, hidden.
        assert!(html.contains(r#"data-depth="2" data-shade="0" hidden style="--depth:2""#));
    }

    #[test]
    fn listing_shades_the_expanded_region_and_escapes_folder_names() {
        let files = vec![
            member(0, "beach.jpg", "<script>", "beach.jpg"),
            shared_file(1, "b.txt", None),
            member(2, "notes.txt", "<script>", "trip/notes.txt"),
        ];

        let html = render_listing(&files, "test-device");

        // The display name is the root's own name, escaped like any text.
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<span class=\"folder-name\"><script>"));

        // Exactly one tree per distinct folder root — the root's own
        // row is the only one keyed with no subfolder path, and its
        // key is the (HTML-escaped) root name, not a path or index.
        assert_eq!(
            html.matches("data-folder-key=\"&lt;script&gt;\"").count(),
            1
        );

        // The expanded root and its visible members carry the shade.
        let count = html.matches("data-shade=\"1\"").count();
        assert!(count >= 2, "folder row and its members are shaded: {count}");
    }

    #[test]
    fn listing_keys_folders_by_root_name_with_same_name_disambiguation() {
        // Two distinct folder roots whose basename is `Photos`: the
        // first-appearance one keeps the bare name, the later one is
        // suffixed — distinct keys, no render index anywhere.
        let files = vec![
            member(0, "beach.jpg", "Photos", "beach.jpg"),
            member(1, "trip.txt", "Backup/Photos", "notes/trip.txt"),
        ];

        let html = render_listing(&files, "test-device");

        assert_eq!(html.matches("data-folder-key=\"Photos\"").count(), 1);
        assert_eq!(html.matches("data-folder-key=\"Photos-2\"").count(), 1);
        // The disambiguation carries into the subfolder keys too.
        assert!(html.contains("data-folder-key=\"Photos-2/notes\""));
        // No render-order key survives.
        assert!(!html.contains("data-folder-key=\"f"));
    }

    #[test]
    fn listing_folder_keys_rank_by_first_appearance_not_render_position() {
        // Whichever filesystem root appears first in the shared list
        // takes the bare key; the later same-named one is suffixed.
        // The subtree ids pin each row's owner (a different physical
        // root per ordering), so the same assertions verify keys that
        // rank by the root's first appearance — the key set
        // {Photos, Photos-2} never shifts with position.
        let photos_first = vec![
            member(0, "beach.jpg", "Photos", "beach.jpg"),
            member(1, "trip.txt", "Backup/Photos", "notes/trip.txt"),
        ];
        // Same two roots, the shared list reversed: the Backup/Photos
        // root now leads (its member carries id 0, its row renders
        // first), so it — not the original owner — keeps the bare name.
        let backup_first = vec![
            member(0, "trip.txt", "Backup/Photos", "notes/trip.txt"),
            member(1, "beach.jpg", "Photos", "beach.jpg"),
        ];

        for files in [&photos_first, &backup_first] {
            let html = render_listing(files, "test-device");

            // The first-appearing root (hosting the first-listed
            // member, subtree "0") holds the bare key in both
            // orderings, on a different physical root each time.
            assert_eq!(folder_row_key(&html, "data-subtree=\"0\""), "Photos");
            assert_eq!(folder_row_key(&html, "data-subtree=\"1\""), "Photos-2");
            assert_eq!(html.matches("data-folder-key=\"Photos\"").count(), 1);
            assert_eq!(html.matches("data-folder-key=\"Photos-2\"").count(), 1);
        }
    }

    /// The `data-folder-key` of the folder row carrying the given
    /// subtree button attribute — the row's key sits in its own
    /// `<li>` tag, before the subtree button inside it.
    fn folder_row_key<'a>(html: &'a str, subtree_attr: &str) -> &'a str {
        let row_end = html.find(subtree_attr).expect("the row must render");
        let marker = "data-folder-key=\"";
        let key_start = html[..row_end]
            .rfind(marker)
            .expect("the row's <li> tag must carry a folder key")
            + marker.len();
        let key_end = key_start + html[key_start..].find('"').expect("terminated attribute");
        &html[key_start..key_end]
    }

    #[test]
    fn listing_never_leaks_absolute_paths_into_the_markup() {
        // Folder keys derive from display names only: no test path
        // (absolute temp paths through `nowhere()`) may appear.
        let files = vec![
            shared_file(0, "loose.txt", None),
            member(1, "beach.jpg", "Photos", "beach.jpg"),
            member(2, "trip.txt", "Backup/Photos", "notes/trip.txt"),
        ];

        let html = render_listing(&files, "test-device");

        assert!(!html.contains("mosquito-nowhere"));
        assert!(!html.contains(nowhere("Photos").to_string_lossy().as_ref()));
        assert!(!html.contains(nowhere("Backup/Photos").to_string_lossy().as_ref()));
        assert!(!html.contains(std::env::temp_dir().to_string_lossy().as_ref()));
    }

    #[test]
    fn listing_folder_checkboxes_carry_their_subtree_ids() {
        // Collapsed subfolder members must appear in the subtree list
        // too, so selecting a folder acts on members nobody can see.
        let files = vec![
            shared_file(0, "recipes.txt", None),
            member(1, "a.txt", "Photographs", "a.txt"),
            member(2, "todo.txt", "Photographs", "notes/todo.txt"),
            member(3, "b.txt", "Other", "b.txt"),
        ];

        let html = render_listing(&files, "test-device");

        assert!(html.contains("data-subtree=\"1,2\""));
        assert!(html.contains("data-subtree=\"3\""));
    }

    #[test]
    fn listing_renders_desktop_style_selection_ui() {
        let files = vec![shared_file(0, "a.txt", None)];

        let html = render_listing(&files, "test-device");

        // Select-all checkbox in the header, per-file checkboxes in rows,
        // using the app's own square-rounded checkbox artwork.
        assert!(html.contains("aria-label=\"Select all shared files\""));
        assert!(html.contains("data-select=\"0\" hidden"));
        assert!(html.contains("icon-checked"));
        // The download button starts hidden: selection is empty and the
        // inline script only reveals it once something is selected.
        assert!(html.contains("id=\"download-selected\" class=\"download-selected\""));
        assert!(!html.contains("download-selected is-visible"));
        // Selection is wired by an inline vanilla script: no framework
        // and no build step anywhere in the page.
        assert!(html.contains("<script>"));
        assert!(html.contains("addEventListener"));
        // Plain per-file download links remain, so downloads work with
        // JavaScript disabled.
        assert!(html.contains("href=\"/download/0\""));
    }

    #[test]
    fn listing_escapes_names_and_links_ids() {
        let files = vec![SharedFile {
            id: FileId(0),
            name: "<script>.txt".into(),
            path: "unused".into(),
            size: 42,
            provenance: None,
        }];

        let html = render_listing(&files, "test-device");

        assert!(html.contains("&lt;script&gt;.txt"));
        assert!(html.contains("href=\"/download/0\""));
        assert!(html.contains("42 B"));
        // Narrowed to the raw name: the page now legitimately contains
        // its own enhancement <script>, so the whole-document check the
        // old assertion made no longer applies.
        assert!(!html.contains("<script>.txt"));
        assert!(
            html.find("class=\"file-count\"").unwrap() < html.find("class=\"file-list\"").unwrap()
        );
        assert!(html.contains("overflow-y: auto"));
    }

    #[test]
    fn listing_escapes_the_device_name() {
        let html = render_listing(&[], "<script>host</script>");

        assert!(html.contains("Shared from &lt;script&gt;host&lt;/script&gt;"));
        assert!(!html.contains("<script>host</script>"));
    }
}
