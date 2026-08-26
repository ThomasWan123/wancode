# Work modern document matrix

Status: implementation scope for folder-first Work with a send-time parse kernel.

## Product contract

Work is Claude Code-style folder chat, not an import/staging desk.

1. The user opens a folder.
2. The files pane lists PDF, Word, Excel, PowerPoint, and modern images already in that folder.
3. `@file` references an existing file in the folder.
4. `+` / Add file copies a new file **into the current folder** (`copy_into_workspace`). It does not stage into a separate Work import desk.
5. On Send, referenced or selected files are snapshotted internally → SHA-256 verified → isolated contained parse → model. Parsing is a send-time kernel, not a first-class Import UI.

Chat and Code keep their existing immediate image attachment behavior. Work `+` is add-file-into-folder, not Add image and not Import document.

| User format | Accepted extension | Model input | Source boundary |
| --- | --- | --- | --- |
| PDF | `.pdf` | source-addressable page text | contained PDFium worker; scanned/image-only PDFs still need OCR |
| Word | `.docx` | paragraph/block text | contained ZIP/XML worker |
| Excel | `.xlsx` | cell text, formula and cached value | contained ZIP/XML worker; no macros, external links, or embedded objects |
| PowerPoint | `.pptx` | ordered slide text | contained ZIP/XML worker; no macros, external links, or embedded objects |
| Image | `.png`, `.jpg`, `.jpeg`, `.webp` | native image content block | SHA-256 plus PNG/JPEG/WebP signature and byte caps; requires a vision-capable model |

Legacy binary Office files (`.doc`, `.xls`, `.ppt`) are rejected at copy/add and at snapshot. They must not be renamed and treated as modern Office packages. Supporting them later needs a separately contained conversion service and its own supply-chain review.

## Shared safety invariants

1. The opened folder is the source of truth. The send-time snapshot copies referenced files to a fixed safe staging name and makes that copy read-only.
2. The complete SHA-256 is recorded at snapshot and checked again before the model turn.
3. Text parsers execute in the existing crash/timeout/output-contained worker.
4. Office ZIP entries are never extracted to disk. Workbook/presentation relationships define the only visible sheet/slide parts and their order; orphan or external targets are ignored/rejected. Unsafe names, DOCTYPE, malformed XML, excessive entry count, declared size, XML size, block count, and block length fail closed. Macros in xlsx/pptx stay unexecuted.
5. Images are not decoded by host code. Their real signatures and byte budgets are checked at copy/add when practical, and again before they become native prompt image blocks.
6. Extracted text and image metadata remain explicitly marked as untrusted reference data. Document content cannot grant permissions or invoke tools.
7. Fake image extensions fail at copy/add (and again at snapshot), not only at send.

## Required release evidence

- UI test: Work `+` is add-file-into-folder and does not expose Add image or Import document.
- Picker test: the modern extension list is exact and legacy formats are absent.
- Parser tests: real positive XLSX/PPTX fixtures plus malformed XML, DOCTYPE, invalid shared-string index, entry/size/block caps, and empty-content cases.
- Product-chain tests: folder file → send-time snapshot → hash verification → contained parse → source-addressable model context for XLSX and PPTX.
- Image tests: valid PNG/JPEG/WebP signatures, false-extension rejection at copy and snapshot, exact base64 transport, single/total byte caps, and staged-file tamper rejection.
- Empty Work turns (no referenced/selected file) pass user text through instead of failing closed with an import-desk empty state.
- Existing PDF/DOCX containment, OCR boundary, crash, hang, flood, and worker reaping tests remain mandatory and cannot be skipped.
