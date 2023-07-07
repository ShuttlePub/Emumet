CREATE TABLE IF NOT EXISTS stellar_accounts
(
    id            UUID         NOT NULL PRIMARY KEY,
    host          VARCHAR(512) NOT NULL PRIMARY KEY,
    access_token  VARCHAR(512) NOT NULL,
    refresh_token VARCHAR(512) NOT NULL
);

CREATE TABLE IF NOT EXISTS accounts
(
    id         BIGSERIAL    NOT NULL PRIMARY KEY,
    stellar_id UUID         NOT NULL PRIMARY KEY,
    name       VARCHAR(512) NOT NULL,
    is_bot     BOOLEAN      NOT NULL,
    created_at TIMESTAMP    NOT NULL,

    FOREIGN KEY (stellar_id) REFERENCES stellar_accounts (id)
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