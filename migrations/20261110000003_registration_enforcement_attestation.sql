-- Legacy control-plane rows have no deployment attestation. Startup must replace
-- this legacy default before mounting any registration route. Old audit payloads
-- are decoded with serde defaults; provider observations cannot overwrite attestation.
ALTER TABLE registration_gate_state
    ADD COLUMN enforcement_verified boolean NOT NULL DEFAULT true,
    ADD COLUMN enforcement_lost_latch boolean NOT NULL DEFAULT false;
ALTER TABLE registration_gate_events DROP CONSTRAINT chk_registration_gate_event_kind;
ALTER TABLE registration_gate_events ADD CONSTRAINT chk_registration_gate_event_kind CHECK (
    kind IN ('outage_started', 'outage_recovered', 'protection_degraded',
        'protection_lost_stopped', 'override_started', 'override_revoked',
        'enforcement_lost_stopped', 'enforcement_restored')
);
ALTER TABLE registration_gate_webhook_deliveries DROP CONSTRAINT chk_webhook_delivery_kind;
ALTER TABLE registration_gate_webhook_deliveries ADD CONSTRAINT chk_webhook_delivery_kind CHECK (
    kind IN ('outage_started', 'outage_recovered', 'protection_degraded',
        'protection_lost_stopped', 'override_started', 'override_revoked',
        'enforcement_lost_stopped', 'enforcement_restored')
);
