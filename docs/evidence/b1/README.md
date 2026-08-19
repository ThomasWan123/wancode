# B1 competitive benchmark evidence

Each release has one machine-validated root JSON record. A complete round also
contains frozen task snapshots, preregistration, redacted transcripts, diffs,
test logs, and blind adjudication under this directory. Missing competitor
access is recorded as `NOT-RUN`; it is never converted into a WanCode win.

Validate a record with:

```powershell
python scripts/validate_b1_evidence.py docs/evidence/b1/v0.20.0.json
```
