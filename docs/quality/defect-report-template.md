# WanCode Defect Report Template

## Identity

- Title:
- Severity: P0 / P1 / P2 / P3
- Component:
- Found by: automated / exploratory / dogfood / customer / competitive benchmark
- WanCode version and commit:
- Engine commit:
- Surface: Chat / Code / startup / updater
- Provider catalog key and backend:
- Operating system and display scale:

## Summary and impact

- One-sentence failure:
- User impact:
- Data/security impact:
- Frequency:
- Safe workaround, if any:

## Preconditions

- Installation state:
- Workspace fixture:
- Session state:
- Network state:
- Relevant configuration with secrets removed:

## Minimal reproduction

1.
2.
3.

## Expected behavior


## Actual behavior


## Evidence

- Screenshot or video:
- Application log:
- ACP/provider transcript:
- Filesystem/Git assertion:
- Session ID:
- Request/correlation ID:
- Crash dump or updater log:

## Recovery and cleanup

- Can the user continue safely?
- Did restart/retry recover?
- Was any state lost or broadened?
- Fixture cleanup confirmed:

## Diagnosis

- Suspected invariant violated:
- First bad / last good version:
- Root cause:
- Adjacent paths reviewed:

## Closure gate

- Fix commit/PR:
- Discriminating regression test:
- Mutation or old-build proof that the test fails before the fix:
- Positive control:
- Full affected suite:
- Documentation or migration impact:
- Quality owner sign-off:
