-- M0-D 专用合成实验；不是生产 schema、HTTP parser 或密码编码。
CREATE TABLE domains (
    scope text[] PRIMARY KEY CHECK (cardinality(scope) = 4),
    restore text NOT NULL DEFAULT 'R1',
    high_water bigint NOT NULL DEFAULT 0 CHECK (high_water >= 0),
    allowed boolean NOT NULL DEFAULT true,
    frozen text,
    epoch integer NOT NULL DEFAULT 1,
    business_limit bigint NOT NULL DEFAULT 100,
    receipt_limit integer NOT NULL DEFAULT 64
);
CREATE TABLE revisions (
    scope text[] NOT NULL REFERENCES domains,
    revision text NOT NULL,
    object_id text NOT NULL,
    body jsonb NOT NULL,
    PRIMARY KEY (scope, revision)
);
CREATE TABLE heads (
    scope text[] NOT NULL REFERENCES domains,
    object_id text NOT NULL,
    revision text NOT NULL,
    deleted boolean NOT NULL,
    PRIMARY KEY (scope, object_id),
    FOREIGN KEY (scope, revision) REFERENCES revisions
);
CREATE TABLE batches (
    scope text[] NOT NULL REFERENCES domains,
    restore text NOT NULL,
    position bigint NOT NULL,
    body jsonb NOT NULL,
    PRIMARY KEY (scope, restore, position)
);
CREATE TABLE receipts (
    scope text[] NOT NULL REFERENCES domains,
    restore text NOT NULL,
    operation text NOT NULL,
    body jsonb NOT NULL,
    result jsonb NOT NULL,
    PRIMARY KEY (scope, restore, operation)
);
CREATE TABLE freezes (
    scope text[] NOT NULL REFERENCES domains,
    restore text NOT NULL,
    identity text NOT NULL,
    upper_bound bigint NOT NULL,
    cancelled boolean NOT NULL DEFAULT false,
    PRIMARY KEY (scope, restore, identity)
);

-- 只有测试调用点使用此屏障；控制器持有 701，pg_blocking_pids 确认到达。
CREATE FUNCTION barrier(wanted text, stage text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF wanted = stage THEN PERFORM pg_advisory_xact_lock(701); END IF;
END $$;

CREATE FUNCTION submit(s text[], r text, op jsonb, pause_at text DEFAULT '')
RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE
    d domains%ROWTYPE;
    prior receipts%ROWTYPE;
    item jsonb;
    actual text;
    outcome jsonb;
    conflict boolean := false;
BEGIN
    -- 实验上限只约束已解析的合成 jsonb；不宣称网络解码的资源保护。
    IF octet_length(op::text) > 4096 THEN RAISE EXCEPTION 'body_limit'; END IF;
    IF jsonb_typeof(op->'pre') IS DISTINCT FROM 'object'
       OR jsonb_typeof(op->'writes') IS DISTINCT FROM 'array'
       OR jsonb_typeof(op->'id') IS DISTINCT FROM 'string'
       OR jsonb_typeof(op->'epoch') IS DISTINCT FROM 'number'
       OR jsonb_typeof(op->'nonce') IS DISTINCT FROM 'string' THEN
        RAISE EXCEPTION 'structure';
    END IF;
    IF jsonb_array_length(op->'writes') NOT BETWEEN 1 AND 4
       OR (SELECT count(*) FROM jsonb_object_keys(op->'pre')) > 8 THEN
        RAISE EXCEPTION 'member_limit';
    END IF;
    IF (SELECT count(DISTINCT v->>'id') FROM jsonb_array_elements(op->'writes') v)
       <> jsonb_array_length(op->'writes') THEN RAISE EXCEPTION 'duplicate'; END IF;
    IF (SELECT count(DISTINCT v->>'rev') FROM jsonb_array_elements(op->'writes') v)
       <> jsonb_array_length(op->'writes') THEN RAISE EXCEPTION 'duplicate_revision'; END IF;
    FOR item IN SELECT * FROM jsonb_array_elements(op->'writes') LOOP
        IF jsonb_typeof(item->'id') IS DISTINCT FROM 'string'
           OR jsonb_typeof(item->'rev') IS DISTINCT FROM 'string'
           OR jsonb_typeof(item->'payload') IS DISTINCT FROM 'string'
           OR jsonb_typeof(item->'deleted') IS DISTINCT FROM 'boolean'
           OR NOT (op->'pre' ? (item->>'id'))
           OR item->'parent' IS DISTINCT FROM op->'pre'->(item->>'id') THEN
            RAISE EXCEPTION 'structure';
        END IF;
    END LOOP;
    SELECT * INTO d FROM domains WHERE scope = s FOR UPDATE;
    IF NOT FOUND OR NOT d.allowed THEN RAISE EXCEPTION 'access'; END IF;
    IF d.restore <> r THEN RAISE EXCEPTION 'restore'; END IF;
    SELECT * INTO prior FROM receipts
        WHERE scope = s AND restore = r AND operation = op->>'id';
    IF FOUND THEN
        IF prior.body <> op THEN RAISE EXCEPTION 'identity_reuse'; END IF;
        RETURN prior.result;
    END IF;
    IF d.frozen IS NOT NULL THEN RAISE EXCEPTION 'frozen'; END IF;
    IF d.epoch <> (op->>'epoch')::integer THEN RAISE EXCEPTION 'epoch'; END IF;
    IF d.high_water = 9223372036854775807 THEN RAISE EXCEPTION 'overflow'; END IF;
    IF d.high_water >= d.business_limit THEN RAISE EXCEPTION 'business_limit'; END IF;
    IF (SELECT count(*) FROM receipts WHERE scope = s AND restore = r)
       + (SELECT count(*) FROM freezes WHERE scope = s AND restore = r)
       >= d.receipt_limit THEN RAISE EXCEPTION 'receipt_limit'; END IF;
    FOR actual IN SELECT jsonb_object_keys(op->'pre') LOOP
        IF (SELECT to_jsonb(h.revision) FROM heads h
            WHERE h.scope = s AND h.object_id = actual)
            IS DISTINCT FROM NULLIF(op->'pre'->actual, 'null'::jsonb) THEN
            conflict := true;
        END IF;
    END LOOP;
    IF conflict THEN
        outcome := '{"status":"preconditionRejected"}';
        INSERT INTO receipts VALUES (s, r, op->>'id', op, outcome);
        RETURN outcome;
    END IF;
    IF EXISTS (SELECT FROM revisions rv, jsonb_array_elements(op->'writes') w
               WHERE rv.scope = s AND rv.revision = w->>'rev') THEN
        RAISE EXCEPTION 'revision_reuse';
    END IF;
    FOR item IN SELECT * FROM jsonb_array_elements(op->'writes') LOOP
        INSERT INTO revisions VALUES (s, item->>'rev', item->>'id', item);
        INSERT INTO heads VALUES (s, item->>'id', item->>'rev', (item->>'deleted')::boolean)
            ON CONFLICT (scope, object_id) DO UPDATE
            SET revision = excluded.revision, deleted = excluded.deleted;
        PERFORM barrier(pause_at, 'head');
    END LOOP;
    INSERT INTO batches VALUES (s, r, d.high_water + 1, op);
    PERFORM barrier(pause_at, 'batch');
    outcome := jsonb_build_object('status', 'committed', 'position', d.high_water + 1,
        'revisions', (SELECT jsonb_agg(v->>'rev') FROM jsonb_array_elements(op->'writes') v));
    INSERT INTO receipts VALUES (s, r, op->>'id', op, outcome);
    PERFORM barrier(pause_at, 'receipt');
    UPDATE domains SET high_water = d.high_water + 1 WHERE scope = s;
    PERFORM barrier(pause_at, 'water');
    RETURN outcome;
END $$;

-- SHARE 与写入 / 恢复控制互斥，整个页内保持代次一致；没有 HTTP token。
CREATE FUNCTION read_page(s text[], r text, applied bigint, upper_bound bigint,
                          budget integer DEFAULT 4096, pause_at text DEFAULT '')
RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE
    d domains%ROWTYPE;
    b batches%ROWTYPE;
BEGIN
    SELECT * INTO d FROM domains WHERE scope = s FOR SHARE;
    IF NOT FOUND OR NOT d.allowed THEN RAISE EXCEPTION 'access'; END IF;
    IF d.restore <> r THEN RAISE EXCEPTION 'restore'; END IF;
    IF applied < 0 OR applied > upper_bound OR upper_bound > d.high_water THEN
        RAISE EXCEPTION 'window';
    END IF;
    PERFORM barrier(pause_at, 'page');
    IF applied = upper_bound THEN RETURN '{"done":true}'; END IF;
    SELECT * INTO b FROM batches WHERE scope = s AND restore = r
        AND position = applied + 1;
    IF NOT FOUND THEN RAISE EXCEPTION 'gap'; END IF;
    IF octet_length(b.body::text) > budget THEN RAISE EXCEPTION 'page_limit'; END IF;
    RETURN jsonb_build_object('position', b.position, 'body', b.body);
END $$;

CREATE FUNCTION freeze_domain(s text[], r text, identity_arg text)
RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE
    d domains%ROWTYPE;
    f freezes%ROWTYPE;
BEGIN
    SELECT * INTO d FROM domains WHERE scope = s FOR UPDATE;
    IF NOT FOUND OR NOT d.allowed THEN RAISE EXCEPTION 'access'; END IF;
    IF d.restore <> r THEN RAISE EXCEPTION 'restore'; END IF;
    SELECT * INTO f FROM freezes WHERE scope = s AND restore = r AND identity = identity_arg;
    IF FOUND THEN RETURN f.upper_bound; END IF;
    IF d.frozen IS NOT NULL THEN RAISE EXCEPTION 'frozen'; END IF;
    IF (SELECT count(*) FROM receipts WHERE scope = s AND restore = r)
       + (SELECT count(*) FROM freezes WHERE scope = s AND restore = r)
       >= d.receipt_limit THEN RAISE EXCEPTION 'receipt_limit'; END IF;
    INSERT INTO freezes VALUES (s, r, identity_arg, d.high_water, false);
    UPDATE domains SET frozen = identity_arg WHERE scope = s;
    RETURN d.high_water;
END $$;

CREATE FUNCTION unfreeze_domain(s text[], r text, identity_arg text)
RETURNS text LANGUAGE plpgsql AS $$
DECLARE
    d domains%ROWTYPE;
    f freezes%ROWTYPE;
BEGIN
    SELECT * INTO d FROM domains WHERE scope = s FOR UPDATE;
    IF NOT FOUND OR NOT d.allowed THEN RAISE EXCEPTION 'access'; END IF;
    IF d.restore <> r THEN RAISE EXCEPTION 'restore'; END IF;
    SELECT * INTO f FROM freezes WHERE scope = s AND restore = r AND identity = identity_arg;
    IF NOT FOUND THEN RAISE EXCEPTION 'freeze_identity'; END IF;
    IF f.cancelled THEN RETURN 'already_cancelled'; END IF;
    IF d.frozen IS DISTINCT FROM identity_arg THEN RAISE EXCEPTION 'freeze_identity'; END IF;
    UPDATE domains SET frozen = NULL WHERE scope = s;
    UPDATE freezes SET cancelled = true WHERE scope = s AND restore = r AND identity = identity_arg;
    RETURN 'cancelled';
END $$;
