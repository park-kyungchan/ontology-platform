-- Additive extension. Deliberately preserves S06 application_id/user_version 1.
CREATE TABLE IF NOT EXISTS local_action_meta (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 version INTEGER NOT NULL CHECK(version=1),
 journal_id TEXT NOT NULL
);
INSERT OR IGNORE INTO local_action_meta VALUES(1,1,lower(hex(randomblob(16))));
CREATE TABLE IF NOT EXISTS local_actions (
 token TEXT PRIMARY KEY,
 instance TEXT COLLATE BINARY NOT NULL,
 account TEXT COLLATE BINARY NOT NULL,
 request_id TEXT COLLATE BINARY NOT NULL,
 object_type TEXT COLLATE BINARY NOT NULL,
 object_key TEXT COLLATE BINARY NOT NULL,
 review TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('reviewed','possible_dispatch','persisted','no_effect','conflict_no_effect','manual_reconciliation')),
 observed TEXT,
 proof TEXT,
 UNIQUE(instance,account,request_id)
);
CREATE TABLE IF NOT EXISTS local_fences (
 instance TEXT COLLATE BINARY NOT NULL,
 account TEXT COLLATE BINARY NOT NULL,
 record TEXT COLLATE BINARY NOT NULL,
 field TEXT COLLATE BINARY NOT NULL,
 token TEXT NOT NULL UNIQUE REFERENCES local_actions(token),
 PRIMARY KEY(instance,account,record,field)
) WITHOUT ROWID;
-- Keep the admitted data decision coherent across the durable fence/target gap.
-- Includes S06 writers: their existing append transaction rolls back, never loses
-- a fence or changes scope/head while an effect might still be in progress.
CREATE TRIGGER IF NOT EXISTS local_action_basis_hold BEFORE UPDATE ON objects
WHEN EXISTS (
 SELECT 1 FROM local_fences f JOIN local_actions a ON a.token=f.token
 WHERE a.instance=OLD.instance AND a.account=OLD.account
 AND a.object_type=OLD.object_type AND a.object_key=OLD.native_key
)
BEGIN SELECT RAISE(ABORT,'local_action_basis_held'); END;
