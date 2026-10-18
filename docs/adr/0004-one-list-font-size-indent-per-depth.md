# One font size for list rows; depth geometry carries the tree

ADR-0002's view drew folder rows a step smaller than file rows (0.875rem vs
1rem) with the hierarchy carried entirely by the alternating shade — the UIs
recorded "never indentation". Unifying on 0.875rem for all **list rows** plus
16px-per-depth geometry (name text indents per depth, the folder chevron
travels with it) serves longer file names, removes the folder/file
"different kind of text" read, and makes the tree readable in space as well
as shade — so the shading comment's "never indentation" wording is
superseded while ADR-0002's derivation and shading scheme are untouched.

Membership reads as: everything inside a folder sits exactly one indent
level right of the folder's own name. Subfolders get there through their
carried chevron; member files catch up the chevron slot (+36px = column +
gap) on top of their own depth. Loose files stay flush; the checkbox, trash,
and size columns never move.

Considered alternatives: keep the two-size split (content-first hierarchy,
but leaves the misalignment and same-size group labels question unsolved);
raise folders to 1rem (shrinks nothing, buys no name space); indent member
files by depth alone (children render left of their parent folder's name —
confusing, since the parent's name starts after the chevron slot). The
smaller font buys ~12% more characters per line, which pays back the indent
steps through depth two.

Consequences: both UIs must mirror this exactly (ADR-0003's duplicated design
tokens extend to the indent step and the chevron carry); the depth token
rides each tree row (`--depth`, plus `data-depth` on member file rows so the
stylesheet can tell them from loose rows); loose rows carry no depth
attributes on either side. Row heights unify on the folder row's 38px strip
— files and the header included, header in the same 0.875rem as the rows —
with the desktop list's 48px touch strip consciously traded for density.
Top-level folders with more than five direct children start collapsed
(mirrored budget), and the enhancement script needs no changes since the
tokens ride the server-rendered rows.

