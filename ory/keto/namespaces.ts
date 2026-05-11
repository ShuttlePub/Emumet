import { Namespace, Context } from "@ory/keto-namespace-types"

class User implements Namespace {}

class Account implements Namespace {
  related: {
    owners: User[]
    editors: User[]
    signers: User[]
  }
  permits = {
    view: (ctx: Context): boolean =>
      this.related.owners.includes(ctx.subject) ||
      this.related.editors.includes(ctx.subject) ||
      this.related.signers.includes(ctx.subject),
    edit: (ctx: Context): boolean =>
      this.related.owners.includes(ctx.subject) ||
      this.related.editors.includes(ctx.subject),
    sign: (ctx: Context): boolean =>
      this.related.owners.includes(ctx.subject) ||
      this.related.signers.includes(ctx.subject),
    deactivate: (ctx: Context): boolean =>
      this.related.owners.includes(ctx.subject),
  }
}

class Instance implements Namespace {
  related: {
    admins: User[]
    moderators: User[]
  }
  permits = {
    moderate: (ctx: Context): boolean =>
      this.related.admins.includes(ctx.subject) ||
      this.related.moderators.includes(ctx.subject),
  }
}
