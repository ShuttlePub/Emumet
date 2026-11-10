// Never forward credentials, request cookies, session tokens, or arbitrary headers.
function(ctx) {
  identity: {
    id: ctx.identity.id,
    traits: { email: ctx.identity.traits.email },
    verifiable_addresses:
      if std.objectHas(ctx.identity, 'verifiable_addresses') then ctx.identity.verifiable_addresses else [],
  },
  flow: {
    id: ctx.flow.id,
    type: ctx.flow.type,
    transient_payload:
      if std.objectHas(ctx.flow, 'transient_payload') then ctx.flow.transient_payload else {},
  },
}
