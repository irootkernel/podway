# Example Session

This walkthrough illustrates the public workflow. IDs and digests are abbreviated.

```bash
podway init
podway start --preset bug-fix --task "fix duplicate login session creation"
podway next
```

The first stage requires a baseline and expected/actual behavior.

```bash
podway check baseline-established
podway set expected-behavior "A login creates exactly one session."
podway set actual-behavior "Concurrent callbacks can create two sessions."
podway attach reproduction-reference tests/login_race.rs
podway complete
```

After diagnosis and regression setup:

```bash
podway set cause "The callback can pass the existence check concurrently."
podway add affected-components auth/session_store
podway complete

podway attach regression-case tests/login_race.rs
podway set regression-behavior "The test fails when two sessions are created."
podway complete
```

Complete implementation, then retry verification after detecting a bad environment:

```bash
podway check implementation-complete
podway set fix-summary "Serialized session creation under the user-scoped lock."
podway complete

podway retry --reason "verification used the wrong feature flags"
podway check original-failure-resolved
podway check regression-check-passed
podway set verification-note "The race test and relevant authentication tests passed."
podway complete
```

Review finds another implementation issue:

```bash
podway return --to fix --reason "review found an unhandled cancellation path"
podway status
```

Expected stage view:

```text
reproduce   done
diagnose    done
regression  done
fix         current
verify      redo
review      redo
finish      pending
```

Perform fix, verification, and review again, then finish:

```bash
podway check implementation-complete
podway set fix-summary "Added cancellation-safe lock release."
podway complete

podway check original-failure-resolved
podway check regression-check-passed
podway set verification-note "Race, cancellation, and authentication suites passed."
podway complete

podway check review-complete
podway check findings-resolved
podway check scope-appropriate
podway complete

podway check task-result-ready
podway set final-summary "Duplicate session creation and cancellation cleanup are fixed."
podway complete
```

The session is completed and remains inspectable until:

```bash
podway reset --yes
```
