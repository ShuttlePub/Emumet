#!/usr/bin/env sh
curl -o 20230707210300_init.dbml https://raw.githubusercontent.com/ShuttlePub/document/9150dd142c6ec981e3af99f7972cc1a1fa850814/packages/document/dbml/emumet.dbml
pnpm --package=@dbml/cli dlx dbml2sql 20230707210300_init.dbml -o 20230707210300_init.sql