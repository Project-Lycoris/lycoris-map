# Isolated Android functional testing

The native `qa` build talks only to the local test gateway at `http://10.0.2.2:18187/`. The emulator maps `10.0.2.2` to the host loopback. The gateway forwards only to `127.0.0.1:18186`, where a separate Rust backend serves a synthetic database. These endpoints have no production fallback.

## Reproduce the backend

Prerequisites: Python 3, a local Unix-socket Docker engine with Compose, and enough memory to build the Rust backend. Select the intended local Docker context before starting. The Compose project is `lycoris-android-qa`; its database, Redis, uploads and containers are independently named. PostgreSQL and Redis have no host-published ports. Only the backend binds host loopback port 18186.

From the repository root:

```sh
python3 apps/android/scripts/qa_setup.py init
python3 apps/android/scripts/qa_setup.py start
python3 apps/android/scripts/qa_gateway.py
```

The last command runs the gateway in the foreground on loopback port 18187. Keep it running during device tests. `start` uses the existing locked Rust Dockerfile with one Cargo build job, applies migrations only to `lycoris_android_qa`, starts dependencies, and creates or reuses three owned synthetic accounts and five fixture markers. It never deletes other users or points, resets existing passwords, restores production data or edits another database.

The setup verifies the local Unix-socket Docker endpoint, fixed Compose project/services,
loopback backend binding, and database identity before creating fixture accounts directly
in the synthetic database. It uses PostgreSQL pgcrypto bcrypt hashes, then the normal HTTP
login endpoint. The three accounts must use the generated `android_qa_` role names and
`example.invalid` emails, always with role USER. This avoids external mail while leaving
production registration verification unchanged. Existing users are checked and logged in;
their passwords and roles are never overwritten. Remote Docker contexts/overrides and
non-fixture identities are rejected. Passwords are sent to psql on stdin and never printed.

For networks requiring a build proxy, set `ANDROID_QA_BUILD_PROXY` before the first `init`, or update that value in the private `app/build/qa/backend.env`. Container builds need a proxy address reachable from the Docker VM, rather than the host's own `127.0.0.1`. Do not use a production database URL; this setup intentionally provides no URL override.

All generated configuration, random synthetic passwords and fixture IDs are stored in `apps/android/app/build/qa/`, which is ignored as build output. `credentials.json` contains fixture-owner, Alice and Bob accounts for login and account-switch tests. Do not copy these credentials into application code or logs. Keep this directory while reusing its Docker volumes. If only account credentials are lost, `init` creates fresh accounts rather than changing existing passwords. If `backend.env` is lost too, restore that local configuration before reusing the database volume; setup does not reset its password or discard its contents.

To check an already running environment:

```sh
python3 apps/android/scripts/qa_setup.py verify
python3 apps/android/scripts/qa_setup.py seed
```

`seed` is repeatable using the persisted credentials and fixed client request IDs. It only approves the exact fixture marker ID, owner public ID and request key that it created. It does not approve arbitrary contributions made during application tests.

## Native preflight protocol

Before any non-GET/HEAD/OPTIONS request—including login or registration—the QA client must verify its exact local origin and obtain:

```http
GET /__lycoris_qa__/manifest
```

```json
{
  "testEnvironment": "lycoris-android-synthetic-v1",
  "protocolVersion": 1,
  "instanceNonce": "<64 lowercase hexadecimal characters>",
  "upstream": "http://127.0.0.1:18186/",
  "sentinel": {
    "markerId": 1,
    "title": "Android QA Environment Sentinel v1",
    "description": "Synthetic Android QA only. Never production data.",
    "ownerPublicId": "<synthetic fixture owner UUID>",
    "clientRequestId": "android-qa-sentinel-v1"
  }
}
```

The marker ID is an example; consume the verified manifest, since a reused database can assign a different ID. A response must contain `X-Lycoris-Test-Environment: lycoris-android-synthetic-v1`. The nonce is generated for each gateway process, so it becomes stale after a gateway restart.

Echo these headers on mutations:

```http
X-Lycoris-Test-Environment: lycoris-android-synthetic-v1
X-Lycoris-Test-Nonce: <instanceNonce>
```

The gateway checks the sentinel's exact title, description, owner, client request ID, public flag and approved status before returning a manifest and again before forwarding each mutation. Missing or stale headers return 403 without forwarding. A missing or mismatched sentinel returns 503. Redirects and absolute external request URLs are rejected. Cookies and empty 200 mutation responses are preserved. The manifest is an accidental-environment protection mechanism for local QA, not an authentication mechanism for a public service.

## Fixtures and checks

Four visible points around WGS84 `31.2304, 121.4737` cover accessible toilets, nursing rooms, medical institutions and other locations. The sentinel is at `-70, 0`, outside the UI fixture area. `fixtures.json` records actual IDs. Images, contribution draft state and permissions are exercised by dedicated app tests rather than pre-populated with production content.

Run bootstrap, gateway and device-runner isolation regressions without a running backend:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s apps/android/scripts -p '*_test.py' -v
```

Use `docker compose --env-file apps/android/app/build/qa/backend.env -f apps/android/scripts/qa.compose.yml stop` when the test stack is no longer needed. This preserves its synthetic database and uploads for later sessions.

## Opt-in native integration runner

After building `:app:assembleQa` and `:app:assembleQaAndroidTest`, starting the isolated backend/gateway,
and booting the intended emulator, run from the repository root:

```sh
python3 apps/android/scripts/qa_device_test.py --serial emulator-5554
```

Replace the serial with the exact intended device. The runner has no default device and never builds,
starts containers, changes fixtures, or falls back to a public endpoint. It installs only
`app/build/outputs/apk/qa/app-qa.apk` and
`app/build/outputs/apk/androidTest/qa/app-qa-androidTest.apk`. Before installation it checks Gradle
output metadata and the APK manifest package names with `aapt2`; after installation it checks that
the fixed instrumentation runner targets `com.lycoris.maps.qa`. SDK tools are discovered from PATH,
`ANDROID_SDK_ROOT` / `ANDROID_HOME`, `local.properties`, or standard SDK directories. Explicit
`--adb /path/to/adb --aapt2 /path/to/aapt2` overrides are available.

Android 17 (SDK 37) blocks local-network connections for apps targeting SDK 37 until
`ACCESS_LOCAL_NETWORK` is granted; this can appear as a TCP timeout. The permission is declared
only in `src/qa/AndroidManifest.xml`, because only QA connects to the host-loopback gateway.
Preview and release use public HTTPS and do not declare this permission. See Android's
[local network permission documentation](https://developer.android.com/privacy-and-security/local-network-permission).

After the isolated environment and fixed APK identities pass preflight, the runner captures the
selected device's SDK and numeric current Android user. Installation and instrumentation explicitly
target that same user. On SDK 37 or newer it reads the installed QA package's runtime permission
entry for that user, including grant state and flags. Missing, ambiguous or unrecognized state
stops the run; it is never interpreted as a denial. An already-granted permission is left alone.
An unfixed denied permission is temporarily granted only to `com.lycoris.maps.qa` for that user,
then revoked in `finally`, including test failures, timeouts after granting and keyboard interrupts.
The original grant state and flags must match after cleanup before the runner can print `passed`.
Fixed permissions are not overridden; no global network, app-compat setting, other permission,
other package or other user's permission is changed. SDK 26–36 do not take this permission path.

Cleanup failures return nonzero even if the test and database checks passed. If the device is
disconnected or the host process is forcibly killed, automatic cleanup cannot be guaranteed;
inspect the exact QA package and Android user reported in the private log before retrying.
These temporary test grants do not validate the end-user permission prompt or permission matrix.

The script reads only the generated Alice account from ignored `app/build/qa/credentials.json`;
the file must have mode `0600` and contain its seeded public ID. Do not put a password on the command
line. Credentials are POSIX-quoted and piped to a noninteractive device shell, rather than included
in the host adb process arguments, shell history, or echoed commands. Logs redact those values and
sensitive header lines. The Android test has its own temporary encrypted cookie file and Keystore
namespace, so it does not clear the QA app UI's saved session.

The only class executed is
`com.lycoris.maps.feature.contributions.QaBackendIntegrationTest`. Its direct instrumentation
arguments are `lycorisQaUsername` and `lycorisQaPassword`; without them ordinary CI runs skip this
live-backend test. The runner treats a skip as failure. It requires exactly one successful test,
the expected class/method, the successful instrumentation exit marker, and all validated receipts.
A zero adb exit code by itself is insufficient.

Before installing/running, the runner reuses `qa_setup.verify_backend()`, checks that the effective
Docker context is local, and validates the gateway manifest against the local fixture sentinel.
The device test also asserts the QA BuildConfig, `.qa` package, fixed `10.0.2.2:18187` origin and
manifest; `ApiClients` repeats the guarded preflight before every mutation. There is no base-URL,
container, database or credential-path override.

The live test verifies real Rust login, encrypted session restoration, favorite add/read/remove,
exact creation-key replay, multi-chunk image upload, identical chunk replay, status reconciliation,
and repeated completion/start. It restores the account's prior favorite state and leaves its new
pending point and photo proposal labeled `Android QA integration <UUID>` for inspection. It never
edits the existing fixture point text or approves/deletes contributions.

After the test, canonical opaque receipt IDs are checked in a `BEGIN TRANSACTION READ ONLY`
transaction against the fixed synthetic PostgreSQL container. This verifies one marker for the
creation key, one upload for the photo key, exactly one image proposal for the new marker, the
matching upload proposal ID/owner, `COMPLETED`, matching received/total bytes, and empty staging
bytes. Repeated `COMPLETED` HTTP receipts alone are not treated as proof of database uniqueness.

Success produces a short JSON object with test count, creation/proposal counts, uploaded byte count
and a relative log path. Failures produce a redacted JSON reason and exit nonzero. Sanitized logs
are mode `0600` under ignored `app/build/qa/device-tests/`. Credentials, cookies and environment
contents are never printed. The runner does not delete the retained synthetic contributions.

Offline safety checks require no device, Docker or backend:

```sh
python3 -B -m unittest discover -s apps/android/scripts -p 'qa_device_test_test.py' -v
```

The runner's offline checks cover shell quoting, log redaction, private Alice selection,
failed/skipped instrumentation rejection, receipt validation, and the read-only uniqueness oracle.
Permission regressions also cover multi-user state parsing, QA-only manifest scope, preflight order,
already-granted preservation, SDK 26–36 no-op behavior, fixed/unknown state rejection, foreground-user
changes, failed or interrupted tests, uncertain grants, cleanup failures and success reporting only
after verified restoration. They simulate commands and do not invoke adb or a backend.
Run the device runner against the chosen emulator to verify its full orchestration;
offline guard checks alone do not establish a device result.
