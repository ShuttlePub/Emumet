// Explicit allowlist: never send password, cookies, session tokens, or arbitrary headers.
function(ctx)
  local transient =
    if std.objectHas(ctx.flow, 'transient_payload') &&
       std.type(ctx.flow.transient_payload) == 'object'
    then ctx.flow.transient_payload else {};
{
  flow_id: ctx.flow.id,
  flow_type: ctx.flow.type,
  email: ctx.identity.traits.email,
  email_proof:
    if std.objectHas(transient, 'email_proof') then transient.email_proof else null,
  bot_token:
    if std.objectHas(transient, 'bot_token') then transient.bot_token else null,
  registration_probe_id:
    if std.objectHas(transient, 'registration_probe_id') then transient.registration_probe_id else null,
}
