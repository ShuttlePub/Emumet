# Emumet

<a href="https://codecov.io/gh/ShuttlePub/Emumet" >
 <img src="https://codecov.io/gh/ShuttlePub/Emumet/branch/main/graph/badge.svg?token=NY4FA3YZPS"/>
 </a>

## Setup

### Services

```shell
podman-compose up -d
```

PostgreSQL, Redis, Ory Kratos, Ory Hydra が起動します。

### Auth: Ory Kratos + Hydra

- **Kratos** (Identity Management): http://localhost:4433
  - Test user: testuser@example.com / testuser
- **Hydra** (OAuth2/OIDC): http://localhost:4444

### Environment

```shell
cp .env.example .env
```

## DB

`podman-compose` で PostgreSQL が起動します。手動起動する場合:

```shell
podman run --rm --name emumet-postgres -e POSTGRES_PASSWORD=develop -p 5432:5432 docker.io/postgres
```

> User: postgres / Password: develop

## Etymology

EMU(Extravehicular Mobility Unit) + Helmet
