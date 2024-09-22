#!/usr/bin/env sh
curl -o 20230707210300_init.dbml https://raw.githubusercontent.com/ShuttlePub/document/47156f657afa75d459f9f557a09253a2a93e6b71/packages/document/dbml/emumet.dbml
pnpm --package=@dbml/cli dlx dbml2sql 20230707210300_init.dbml -o 20230707210300_init.sql