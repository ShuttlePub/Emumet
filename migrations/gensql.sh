#!/usr/bin/env sh
curl -o 20230707210300_init.dbml https://raw.githubusercontent.com/ShuttlePub/document/6f36e9cd0eb1d2ec46f06eb9daab52c108e625fd/packages/document/dbml/emumet.dbml
pnpm --package=@dbml/cli dlx dbml2sql 20230707210300_init.dbml -o 20230707210300_init.sql