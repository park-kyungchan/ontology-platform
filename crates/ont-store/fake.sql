PRAGMA application_id = 1330533431;
PRAGMA user_version = 1;
CREATE TABLE meta (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 identity TEXT NOT NULL,
 journal_id TEXT NOT NULL
);
CREATE TABLE cells (
 instance TEXT COLLATE BINARY NOT NULL,
 account TEXT COLLATE BINARY NOT NULL,
 record TEXT COLLATE BINARY NOT NULL,
 field TEXT COLLATE BINARY NOT NULL,
 scope TEXT NOT NULL,
 value TEXT NOT NULL,
 version INTEGER NOT NULL CHECK(version>=0),
 PRIMARY KEY(instance,account,record,field)
) WITHOUT ROWID;
CREATE TABLE dispatches (
 token TEXT PRIMARY KEY,
 review TEXT NOT NULL,
 outcome TEXT NOT NULL CHECK(outcome IN ('applied','sealed_no_effect','conflict_no_effect')),
 version INTEGER NOT NULL,
 value TEXT NOT NULL
);
