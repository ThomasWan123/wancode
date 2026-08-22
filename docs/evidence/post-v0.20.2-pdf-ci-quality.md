# Post-v0.20.2 PDF and mandatory document-gate evidence

Date: 2026-08-22 (Asia/Shanghai)

Status: implemented after the published v0.20.2 tag and **not released**. This
document describes the current review branch only; it does not change the
historical claims for v0.20.2.

## Acceptance targets

| Target | Implementation | Required evidence |
|---|---|---|
| Text PDF support | PDFium-backed extraction in the same bounded, crash-contained worker used for DOCX; one source-addressable block per non-empty page | Valid PDF worker and import-to-model-context positive controls |
| Honest OCR boundary | An image-only PDF is rejected with an explicit message that OCR is not supported | Image-only PDF negative test |
| Source integrity | Imported PDF and DOCX files are copied to fixed safe staging names and SHA-256 is checked again before every model turn | PDF and DOCX staged-file tamper tests |
| Mandatory real DOCX gate | A fixed real professional DOCX fixture is committed as base64 and always decoded by the integration test | No environment variable and no skip branch in the CI test |
| Complete chaos matrix | Valid PDF, malformed PDF, real DOCX, mixed context, abort, hang/kill, malformed output, output flood, oversized input, missing input, containment refusal, tamper rejection, and worker reap | `work_parse_containment` must report zero failures |
| Packaged runtime | The pinned PDFium DLL and license are bundled as Tauri resources; release automation fetches and verifies the locked archive and DLL hashes before building | PDFium verify-only gate plus local NSIS build/install inspection |

## Current local evidence

| Gate | Result |
|---|---|
| Mandatory document/containment matrix | 21 passed, 0 failed; includes independent page-count, per-page-text, and total-text cap probes |
| Fixed real DOCX | 40 well-formed blocks; import-to-context passed |
| Generated standards-compliant text PDF | worker extraction and import-to-context passed |
| Image-only PDF | rejected with the documented OCR boundary |
| Mixed PDF + DOCX context | complete, source-addressable context produced |
| Optional independent real-world PDF probe | passed; complete matrix 22 passed, 0 failed, including 2 pages with extractable text |
| Full Rust library suite | 332 passed, 0 failed, 1 intentionally ignored CI-only probe |
| Exact repository Rust CI command | passed: library suite, engine canary 8/8, model block 1/1, provider compliance 1/1 + 1/1, Work surface engine, job breakaway, and mandatory document/containment 21/21 |
| Frontend tests and production build | 16 files / 85 tests passed; production build passed |
| Clippy with warnings denied | debug and release profiles passed for the `wancode` crate (`--no-deps -D warnings`) |
| PDFium supply-chain verification | passed; version `153.0.7999.0`, 7,260,672 bytes, SHA-256 `fb898a1f5ace57805834f390407500bdb6ef93eff326a252ad334a8aae809d8e` |
| Local NSIS build | passed from final source; 29,202,190 bytes, SHA-256 `6a037017494e08e59a167faa507b3d6fa3e4502b4b4c98f73d5ef47d76a88ad5` |
| Isolated package inspection | passed; install exit 0, installed DLL hash matched the lock, license was present, and the installed worker extracted 2 source-addressable pages from an independent real-world PDF |
| Isolated uninstall cleanup | passed; uninstall exit 0 and the test install directory was removed |

No release, tag, asset upload, updater-manifest publication, or merge is covered
by this evidence. Each remains subject to the repository's explicit
authorization and review protocol.
