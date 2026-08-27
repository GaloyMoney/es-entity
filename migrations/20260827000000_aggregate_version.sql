-- Aggregate version for nested-entity roots.
--
-- Every aggregate root — a repo with #[es_repo(nested)] fields and no `parent`
-- column — carries a `version` on its index table. It is the clock for the
-- whole aggregate: each aggregate write CAS-bumps it, and nested reads
-- re-check it after the children have loaded.
--
-- Mid-level repos in grandchildren trees (a `parent` column AND nested
-- children of their own, e.g. gc_children) are parents but NOT roots, and
-- deliberately get no version column.
--
-- Consumers adopting this need the equivalent statement against each of their
-- own root index tables:
--   ALTER TABLE <roots> ADD COLUMN version INT NOT NULL DEFAULT 1;
-- The DEFAULT makes it safe to apply ahead of the code deploy: existing rows
-- become version 1 and old code ignores the column entirely.

-- Orders: nested `items` (OrderItems), no parent column.
ALTER TABLE orders ADD COLUMN version INT NOT NULL DEFAULT 1;

-- Accounts: nested `holders` (AccountHolders), no parent column.
ALTER TABLE accounts ADD COLUMN version INT NOT NULL DEFAULT 1;

-- GcParents: root of the parent -> child -> grandchild tree.
-- gc_children is intentionally skipped: it has a `parent_id` column, so it is
-- a mid-level parent rather than a root.
ALTER TABLE gc_parents ADD COLUMN version INT NOT NULL DEFAULT 1;
