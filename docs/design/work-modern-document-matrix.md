# Work modern document matrix

Status: implementation scope for the unified **Add document** entry.

## Product contract

The Work header action and the Work composer `+` menu invoke the same picker.
The label stays format-neutral because the action is a document workspace
operation, not an image-only attachment shortcut. Chat and Code keep their
existing immediate image attachment behavior.

| User format | Accepted extension | Model input | Source boundary |
| --- | --- | --- | --- |
| PDF | `.pdf` | source-addressable page text | contained PDFium worker; scanned/image-only PDFs still need OCR |
| Word | `.docx` | paragraph/block text | contained ZIP/XML worker |
| Excel | `.xlsx` | cell text, formula and cached value | contained ZIP/XML worker; no macros, external links, or embedded objects |
| PowerPoint | `.pptx` | ordered slide text | contained ZIP/XML worker; no macros, external links, or embedded objects |
| Image | `.png`, `.jpg`, `.jpeg`, `.webp` | native image content block | SHA-256 plus PNG/JPEG/WebP signature and byte caps; requires a vision-capable model |

Legacy binary Office files (`.doc`, `.xls`, `.ppt`) are rejected. They must not
be renamed and treated as modern Office packages. Supporting them later needs a
separately contained conversion service and its own supply-chain review.

## Shared safety invariants

1. The user file is copied to a fixed safe staging name and made read-only.
2. Its complete SHA-256 is recorded and checked again before every model turn.
3. Text parsers execute in the existing crash/timeout/output-contained worker.
4. Office ZIP entries are never extracted to disk. Unsafe names, DOCTYPE,
   malformed XML, excessive entry count, declared size, XML size, block count,
   and block length fail closed.
5. Images are not decoded by host code. Their real signatures and byte budgets
   are checked before they become native prompt image blocks.
6. Extracted text and image metadata remain explicitly marked as untrusted
   reference data. Document content cannot grant permissions or invoke tools.

## Required release evidence

- UI test: Work `+` calls the shared Add document action and does not expose the
  old Add image action.
- Picker test: the modern extension list is exact and legacy formats are absent.
- Parser tests: real positive XLSX/PPTX fixtures plus malformed XML, DOCTYPE,
  invalid shared-string index, entry/size/block caps, and empty-content cases.
- Product-chain tests: import -> fixed staging -> hash verification -> contained
  parse -> source-addressable model context for XLSX and PPTX.
- Image tests: valid PNG/JPEG/WebP signatures, false-extension rejection, exact
  base64 transport, single/total byte caps, and staged-file tamper rejection.
- Existing PDF/DOCX containment, OCR boundary, crash, hang, flood, and worker
  reaping tests remain mandatory and cannot be skipped.
