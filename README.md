# Emumet

<a href="https://codecov.io/gh/ShuttlePub/Emumet" > 
 <img src="https://codecov.io/gh/ShuttlePub/Emumet/branch/main/graph/badge.svg?token=NY4FA3YZPS"/> 
 </a>

# Keycloak

```shell
mkdir -p keycloak-data/h2
podman run --rm -it -v ./keycloak-data/h2:/opt/keycloak/data/h2:Z,U -v ./keycloak-data/import:/opt/keycloak/data/import:Z,U -p 18080:8080 -e KC_BOOTSTRAP_ADMIN_USERNAME=admin -e KC_BOOTSTRAP_ADMIN_PASSWORD=admin --name emumet-keycloak quay.io/keycloak/keycloak:26.1 start-dev --import-realm
```

> - Url: http://localhost:18080
> - ユーザー名: admin
> - パスワード: admin
> - realm: emumet

### Update realm data

```shell
mkdir -p keycloak-data/export
podman run --rm -v ./keycloak-data/h2:/opt/keycloak/data/h2:Z,U -v ./keycloak-data/export:/opt/keycloak/data/export:Z,U quay.io/keycloak/keycloak:latest export --dir /opt/keycloak/data/export --users same_file --realm emumet
sudo cp keycloak-data/export/* keycloak-data/import/
```

> keycloak本体を停止してから実行してください

# DB

Podman(docker)にて環境構築が可能です

```shell
podman run --rm --name emumet-postgres -e POSTGRES_PASSWORD=develop -p 5432:5432 docker.io/postgres
```

> ユーザー名: postgres
> パスワード: develop

# 語源

EMU(Extravehicular Mobility Unit=宇宙服)+Helmet
