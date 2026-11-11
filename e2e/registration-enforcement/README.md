# Real Kratos registration enforcement (D15)

Linux + Docker Compose v2 (rootful CI), or Podman host networking locally. Dedicated ports: DB 15448, Kratos public/admin 19433/19434, Emumet public/hook 19480/19481, all loopback. Never point this fixture at shared state. Database data is tmpfs; no named volumes. The test uses synthetic credentials/addresses only.

Run `bash e2e/registration-enforcement/run.sh`. It starts PostgreSQL + Kratos v1.3.1 and runs `registration_guard::kratos_e2e::real_kratos_d15_q01_through_q11` with `--ignored` and explicit opt-in. **Success requires 1 passed / 0 failed / 0 ignored**. Outside this fixture the integration test is intentionally ignored, never counted as acceptance. A failed assertion cannot become a skip.

Both Kratos migrate/serve images are pinned to the same verified v1.3.1 OCI digest `sha256:c47903ba278bbcb330b5af4ce43d33dc17adacc1d40456b8e2a235add8ababa5` (E15b recorded manifest + locally pulled RepoDigest; exercised on Linux/amd64). The container engine verifies image content against that digest on pull; the renderer tests reject tag-only or divergent references.

The suite starts actual Emumet production root/hook routers on TCP, real SMTP and Siteverify protocol doubles, and an E15w Webhook receiver. Registration success/denial must agree with both Kratos admin API and identity SQL rows. The production config renderer and hook body are reused; only fixture addresses and HaveIBeenPwned lookup are changed. Restarting Kratos really removes/restores the method hook. Emumet restart reconstructs running routers against the same persisted database.

Protected browser submissions use JSON transient_payload; a traditional form without the required proof/token must fail closed. Browser JSON/API successes and direct form denial are all tested.

Each checkpoint printed below is a scenario in that single ordered integration test (not a separate Rust test count):

| Decision | Checkpoint / actual registration assertions |
| --- | --- |
| q01 | `q01_either_email_or_bot_and_both_effective`: email-only, BOT-only, both effective, no effective protection; missing proofs/tokens; browser JSON/form and API |
| q02 | `q02_config_loss_stops`: configurations disabled, registration denied, reopen rejected |
| q03 | `q03_manual_reopen_and_old_flow_closure`: restored configuration stays latched; explicit reopen; pre-existing flow denied after closure; recovery never removes manual closure |
| q04 | `q04_no_public_invitation_bypass_no_invitation_feature`: no invitation feature added; public registration has no claimed invitation/override exemption. Targeted invitation workflow remains outside this unit |
| q05 | `q05_api_verified_readiness`: populated configuration without business verification cannot open/register; neither configuration nor the periodic worker opens the gate or registration; the administrator's **input-less** verification (real SMTP canary DATA acceptance, Siteverify rejecting the dummy token as `invalid-input-response`) precedes successful registration; the retired token/code inputs are 400 |
| q06 | `q06_temporary_outage_auto_resume`: both providers temporarily fail, registration denied; response evidence restores acceptance without an admin reopen |
| q07 | `q07_single_provider_outage_continues`: failure in either direction enforces only the working side, never no protection |
| q08 | `q08_login_and_authenticated_account_additions`: actual password login while closed; two authenticated Account creates |
| q09 | `q09_warning_ack_required`: missing acknowledgement rejected, explicit unprotected override succeeds |
| q10 | `q10_override_persists_until_revoked_and_enforces_ready_protection`: restart/recovery preserves metadata; ready protection still required; explicit revoke |
| q11 | `q11_persistent_notifications_unread_and_e15w_webhook`: outage/recovery/degradation/config-loss/enforcement-loss+restoration/override events in DB-backed unread list and external E15w delivery |

Also: provider timeout bounded fail-closed; `enforcement_hook_removed_negative_canary_restart_manual_reack` creates and cleans reserved identities, persists stop independently of override/provider readiness, survives Emumet restart, requires renewed warning to resume unprotected. Hook receipt matching prevents native-validation failure from masquerading as attestation.

Rejected email addresses are retained across every scenario. After provider timeout and again at suite completion, the test first waits for the counted hook requests to finish, then repeatedly checks **both admin API and identity SQL** for every rejected address for at least **10 seconds** (two five-second hook deadlines, also longer than the six-second synthetic Siteverify stall). This is bounded late-persist observation, not a claim about arbitrarily delayed database work. The q08 regressions assert successful login + two Account additions in **both manual-closed and provider-outage states**, with **hook call increment exactly zero** around each set of operations.

Logs: `result.log` and CI artifact `registration-enforcement-real-kratos`. The script deliberately does not tear down implicitly. Cleanup separately:

`docker compose -p emumet-registration-enforcement-e15d -f e2e/registration-enforcement/compose.yml down`

Use the same engine as startup; no `-v`. Docker rootless host networking may not be the actual host namespace; use Podman locally or the documented rootful CI. See [operational limitations](../../docs/registration-enforcement.md): finite LB sampling is not replica admission or physical shutdown.
