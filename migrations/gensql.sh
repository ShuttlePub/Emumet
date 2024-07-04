#!/usr/bin/env sh
curl -o 20230707210300_init.dbml https://raw.githubusercontent.com/ShuttlePub/document/b05e336ad10a94a429e7afa13f5a6ac8f4dc0871/packages/document/dbml/emumet.dbml
pnpm --package=@dbml/cli dlx dbml2sql 20230707210300_init.dbml -o 20230707210300_init.sql