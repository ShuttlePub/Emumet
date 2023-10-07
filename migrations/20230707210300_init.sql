CREATE TABLE IF NOT EXISTS stellar_accounts
(
    id            BIGSERIAL    NOT NULL PRIMARY KEY,
    host          VARCHAR(512) NOT NULL,
    client_id     VARCHAR(512) NOT NULL,
    access_token  VARCHAR(512) NOT NULL,
    refresh_token VARCHAR(512) NOT NULL
);

CREATE TABLE IF NOT EXISTS accounts
(
    id         BIGSERIAL    NOT NULL PRIMARY KEY,
    domain     TEXT         NOT NULL,
    name       VARCHAR(512) NOT NULL,
    is_bot     BOOLEAN      NOT NULL,
    created_at TIMESTAMP    NOT NULL,
    UNIQUE (domain, name)
);
CREATE TABLE IF NOT EXISTS stellar_emumet_accounts
(
    stellar_id BIGSERIAL NOT NULL,
    emumet_id  BIGSERIAL NOT NULL,
    PRIMARY KEY (stellar_id, emumet_id),
    FOREIGN KEY (stellar_id) REFERENCES stellar_accounts (id),
    FOREIGN KEY (emumet_id) REFERENCES accounts (id)
);

CREATE TABLE IF NOT EXISTS profiles
(
    account_id   BIGSERIAL NOT NULL PRIMARY KEY,
    display_name VARCHAR(64),
    summary      TEXT,
    icon         VARCHAR(256),
    banner       VARCHAR(256),

    FOREIGN KEY (account_id) REFERENCES accounts (id)
);

CREATE TABLE IF NOT EXISTS metadatas
(
    id         BIGSERIAL PRIMARY KEY,
    account_id BIGSERIAL    NOT NULL,
    label      VARCHAR(64)  NOT NULL,
    content    VARCHAR(256) NOT NULL,

    FOREIGN KEY (account_id) REFERENCES accounts (id)
);

CREATE TABLE IF NOT EXISTS remote_accounts
(
    id  BIGSERIAL PRIMARY KEY,
    url VARCHAR(512) NOT NULL
);

CREATE TABLE IF NOT EXISTS follows
(
    id                 BIGSERIAL PRIMARY KEY,
    source_local       BIGSERIAL,
    source_remote      BIGSERIAL,
    destination_local  BIGSERIAL,
    destination_remote BIGSERIAL,

    FOREIGN KEY (source_local) REFERENCES accounts (id),
    FOREIGN KEY (source_remote) REFERENCES remote_accounts (id),
    FOREIGN KEY (destination_local) REFERENCES accounts (id),
    FOREIGN KEY (destination_remote) REFERENCES remote_accounts (id)
);