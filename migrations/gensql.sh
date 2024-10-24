#!/usr/bin/env sh
curl -o 20230707210300_init.dbml https://raw.githubusercontent.com/ShuttlePub/document/afd96a4bf572786e336ed43ec7c8ad5cc0696bad/packages/document/dbml/emumet.dbml
pnpm --package=@dbml/cli dlx dbml2sql 20230707210300_init.dbml -o 20230707210300_init.sql