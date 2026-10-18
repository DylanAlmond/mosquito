# One font size for list rows; depth geometry carries the tree

ADR-0002's view drew folder rows a step smaller than file rows (0.875rem vs
1rem) with the hierarchy carried entirely by the alternating shade — the UIs
recorded "never indentation". Unifying on 0.875rem for all **list rows** plus
a 16px-per-depth indent on the row's **name text** (controls keep their grid
columns; row heights stay 48px) equally serves longer file names, removes the
folder/file "different kind of text" read, and makes member rows align under
their parents — so the shading comment's "never indentation" wording is
superseded while ADR-0002's derivation and shading scheme are untouched.

Considered alternatives: keep the two-size split (content-first hierarchy,
but leaves the misalignment and same-size group labels question unsolved);
raise folders to 1rem (shrinks nothing, buys no name space). The smaller font
buys ~12% more characters per line, which per-depth indent steps up to level
two more than pay back.

Consequences: both UIs must mirror this exactly (ADR-0003's duplicated design
tokens extend to the indent step); loose files stay flush; the touch-target
row heights are unchanged; the enhancement script needs no changes since the
depth token rides on each row.
