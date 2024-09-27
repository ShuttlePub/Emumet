# Emumet
<a href="https://codecov.io/gh/ShuttlePub/Emumet" > 
 <img src="https://codecov.io/gh/ShuttlePub/Emumet/branch/main/graph/badge.svg?token=NY4FA3YZPS"/> 
 </a>

# DB
Podman(docker)にて環境構築が可能です

```shell
podman run --rm --name emumet-postgres -e POSTGRES_PASSWORD=develop -p 5432:5432 docker.io/postgres
```

> ユーザー名: postgres
> パスワード: develop

# 語源
EMU(Extravehicular Mobility Unit=宇宙服)+Helmet
