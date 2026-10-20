# Notice sources

The collector reads license files from the selected package graph.
It also accepts the exact package versions in `index.json`.
Each added text has a source URL and SHA-256.
The defmt-parser MIT text has one added final LF in this repository.
Its entry records the upstream hash separately as `sourceSha256`.
The stored file hash remains the required collection check.
The collector checks the package name, version, repository, commit, path, and SPDX value.
It checks the original package authors when the entry lists them.
The collector rejects different versions, changed sources, and changed texts.
It does not download files during collection.

## Repository license texts

The added files come from the commits in each registry package's `.cargo_vcs_info.json`.
The registry package metadata provides the repository URL.
The manifest records both values.
Repository license texts keep their original copyright notices.
`plist` also has a `LICENCE` file in its package archive.
The collector reads this file directly when it is present.

## selectors 0.38.0

The fixed `stylo` commit has no complete MPL license text.
The added MPL 2.0 text comes from Mozilla's official text endpoint.
The source notice copies lines 1 through 3 of `selectors/lib.rs` at the fixed commit.
The output includes the package source URL and its crates.io source archive URL.
This text collection does not confirm compliance with MPL source requirements.

## Five objc2 packages

These entries apply only to `block2` 0.6.2, `objc2` 0.6.5, `objc2-encode` 4.1.0,
`dispatch2` 0.3.1, and `objc2-exception-helper` 0.1.1.
Their fixed repository commits contain `LICENSE.md` declarations, but no separate complete license or copyright text.
The output labels each original document as `upstream declaration`.
It keeps the complete Apple SDK discussion from that document.
The output also labels the linked official license texts as `linked license terms`.
These are the texts that the upstream declaration links, not new package-specific copyright grants.
The MIT text keeps `<YEAR>` and `<COPYRIGHT HOLDER>` without substitutions.
The Zlib text keeps its own original names and date without substitutions.
The output lists package authors from the original Cargo metadata.
Authors are not converted into an invented copyright notice.

The MIT text contains the four license paragraphs from the official OSI page.
HTML tags are removed and character references are decoded.
The Zlib text contains the complete `PRE` block from the official linked page.
The Apache text comes from the official text endpoint.
No license words or copyright fields are changed.

This collection makes the source documents available for review.
It does not confirm legal compliance or resolve the upstream SDK questions.
