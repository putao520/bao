// @trace TEST-E2E-DATA [req:REQ-ENG-006,REQ-ENG-007]
// Real-world E2E tests: data pipeline scenarios using Bao as a library.
// Covers: CSV->JSON, JSON aggregation, chunked Buffer streaming, data cleaning,
// and multi-source join. Uses fs/JSON/Buffer/process APIs via install_all.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use std::fs;
use std::path::PathBuf;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

fn escape_path(p: &str) -> String {
    p.replace('\\', "\\\\").replace('"', "\\\"")
}

// Single #[test] — mozjs Runtime is per-thread singleton.
#[test]
fn test_realworld_data_pipeline_all() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // Build per-pid temp directory: /tmp/bao_pipeline_test_<pid>
    let pid = std::process::id();
    let mut dir = std::env::temp_dir();
    dir.push(format!("bao_pipeline_test_{}", pid));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");

    let d = escape_path(&dir.to_string_lossy());

    // Fixture file paths
    let csv_in = dir.join("input.csv");
    let csv_p = escape_path(&csv_in.to_string_lossy());
    let json_in = dir.join("input.json");
    let _json_p = escape_path(&json_in.to_string_lossy()); // path reserved for future use
    let json_out = dir.join("output.json");
    let json_op = escape_path(&json_out.to_string_lossy());
    let agg_out = dir.join("aggregated.json");
    let agg_op = escape_path(&agg_out.to_string_lossy());
    let big_bin = dir.join("big.bin");
    let big_p = escape_path(&big_bin.to_string_lossy());
    let chunk_out = dir.join("chunked.out");
    let chunk_op = escape_path(&chunk_out.to_string_lossy());
    let dirty_in = dir.join("dirty.json");
    let dirty_p = escape_path(&dirty_in.to_string_lossy());
    let clean_out = dir.join("clean.json");
    let clean_op = escape_path(&clean_out.to_string_lossy());
    let users_in = dir.join("users.json");
    let users_p = escape_path(&users_in.to_string_lossy());
    let orders_in = dir.join("orders.json");
    let orders_p = escape_path(&orders_in.to_string_lossy());
    let join_out = dir.join("joined.json");
    let join_op = escape_path(&join_out.to_string_lossy());

    // Seed fixtures from Rust (so test is deterministic even if fs API has gaps).
    fs::write(
        &csv_in,
        "id,name,amount\n1,Alice,100\n2,Bob,200\n3,Alice,150\n4,Bob,50\n5,Charlie,300\n",
    )
    .expect("seed csv");
    fs::write(
        &json_in,
        "[{\"id\":1,\"amount\":10},{\"id\":2,\"amount\":20},{\"id\":3,\"amount\":30}]",
    )
    .expect("seed json");
    fs::write(
        &dirty_in,
        "[{\"id\":1,\"name\":\"A\",\"amount\":\"10\"},{\"id\":2,\"name\":\"\",\"amount\":\"20\"},\
         {\"id\":3,\"name\":\"C\",\"amount\":\"NaN\"},{\"id\":4,\"name\":\"D\",\"amount\":\"40\"}]",
    )
    .expect("seed dirty");
    fs::write(
        &users_in,
        "[{\"uid\":1,\"name\":\"Alice\"},{\"uid\":2,\"name\":\"Bob\"},{\"uid\":3,\"name\":\"Charlie\"}]",
    )
    .expect("seed users");
    fs::write(
        &orders_in,
        "[{\"oid\":101,\"uid\":1,\"total\":50},{\"oid\":102,\"uid\":2,\"total\":75},\
         {\"oid\":103,\"uid\":1,\"total\":25},{\"oid\":104,\"uid\":999,\"total\":999}]",
    )
    .expect("seed orders");

    // ═══════════════════════════════════════════════════════════════
    // Scenario 1: CSV → JSON conversion
    // ═══════════════════════════════════════════════════════════════
    let s1 = eval_string(&mut ctx, &format!(r#"
        var results = [];
        try {{
            var fs = require('fs');
            var raw = fs.readFileSync("{csv_p}", "utf8");
            results.push("csv_read_" + (typeof raw === "string" ? "ok" : "fail"));
            var lines = raw.split(/\r?\n/).filter(function(l) {{ return l.length > 0; }});
            results.push("csv_lines_" + lines.length);
            var header = lines[0].split(",");
            results.push("csv_header_" + header.join("|"));
            var records = [];
            for (var i = 1; i < lines.length; i++) {{
                var parts = lines[i].split(",");
                var obj = {{}};
                for (var j = 0; j < header.length; j++) {{
                    obj[header[j]] = parts[j];
                }}
                records.push(obj);
            }}
            results.push("csv_records_" + records.length);
            results.push("csv_first_name_" + records[0].name);
            results.push("csv_last_amount_" + records[records.length - 1].amount);
            // coerce amount to number, write out as JSON array
            var enriched = records.map(function(r) {{
                return {{ id: Number(r.id), name: r.name, amount: Number(r.amount) }};
            }});
            var total = enriched.reduce(function(acc, r) {{ return acc + r.amount; }}, 0);
            results.push("csv_total_" + total);
            fs.writeFileSync("{json_op}", JSON.stringify(enriched));
            var verify = JSON.parse(fs.readFileSync("{json_op}", "utf8"));
            results.push("csv_json_len_" + verify.length);
            results.push("csv_json_first_" + verify[0].name);
            results.push("CSV_DONE");
        }} catch(e) {{
            results.push("CSV_ERR:" + (e.message || e));
        }}
        results.join("|");
    "#, csv_p = csv_p, json_op = json_op));

    println!("scenario1: {}", s1);
    assert!(s1.contains("CSV_DONE"), "CSV→JSON must complete: {}", s1);
    assert!(s1.contains("csv_lines_6"), "6 lines (1 header + 5 data): {}", s1);
    assert!(s1.contains("csv_records_5"), "5 records: {}", s1);
    assert!(s1.contains("csv_first_name_Alice"), "first record name: {}", s1);
    assert!(s1.contains("csv_total_800"), "sum 100+200+150+50+300=800: {}", s1);
    assert!(s1.contains("csv_json_len_5"), "output JSON length: {}", s1);

    // Verify from Rust side too
    let out1 = fs::read_to_string(&json_out).expect("read output.json");
    assert!(out1.contains("\"name\":\"Alice\""), "Rust verify output.json: {}", out1);

    // ═══════════════════════════════════════════════════════════════
    // Scenario 2: JSON aggregation — group by name, compute count/sum/avg
    // ═══════════════════════════════════════════════════════════════
    let s2 = eval_string(&mut ctx, &format!(r#"
        var results = [];
        try {{
            var fs = require('fs');
            var data = JSON.parse(fs.readFileSync("{json_op}", "utf8"));
            results.push("agg_in_len_" + data.length);
            var groups = {{}};
            for (var i = 0; i < data.length; i++) {{
                var r = data[i];
                var key = r.name;
                if (!groups[key]) groups[key] = {{ count: 0, sum: 0, items: [] }};
                groups[key].count++;
                groups[key].sum += r.amount;
                groups[key].items.push(r);
            }}
            var agg = [];
            var keys = Object.keys(groups);
            for (var k = 0; k < keys.length; k++) {{
                var g = groups[keys[k]];
                agg.push({{ name: keys[k], count: g.count, sum: g.sum, avg: g.sum / g.count }});
            }}
            agg.sort(function(a, b) {{ return a.name < b.name ? -1 : 1; }});
            fs.writeFileSync("{agg_op}", JSON.stringify(agg));
            results.push("agg_groups_" + agg.length);
            results.push("agg_alice_" + agg[0].name + "_" + agg[0].count + "_" + agg[0].sum);
            results.push("agg_bob_sum_" + (function() {{
                for (var i = 0; i < agg.length; i++) if (agg[i].name === "Bob") return agg[i].sum;
                return -1;
            }})());
            results.push("AGG_DONE");
        }} catch(e) {{
            results.push("AGG_ERR:" + (e.message || e));
        }}
        results.join("|");
    "#, json_op = json_op, agg_op = agg_op));

    println!("scenario2: {}", s2);
    assert!(s2.contains("AGG_DONE"), "aggregation must complete: {}", s2);
    assert!(s2.contains("agg_groups_3"), "3 groups (Alice/Bob/Charlie): {}", s2);
    assert!(s2.contains("agg_alice_Alice_2_250"), "Alice: 2 records, sum 250: {}", s2);
    assert!(s2.contains("agg_bob_sum_250"), "Bob sum 200+50=250: {}", s2);

    // ═══════════════════════════════════════════════════════════════
    // Scenario 3: Chunked Buffer streaming — build binary, encode, decode
    //
    // NOTE on findings: `fs.writeFileSync(path, Buffer)` and `fs.appendFileSync`
    // currently ignore non-string data and write empty files (see node_fs.rs
    // fs_write_file_sync: only `data_val.is_string()` branch writes content).
    // To test Buffer chunked logic end-to-end under the current API, we encode
    // each chunk to a hex string line, write text, then read back and decode.
    // This exercises: Buffer.alloc, Buffer indexing, Buffer.concat (if present),
    // hex encoding/decoding, fs write/read of text.
    // ═══════════════════════════════════════════════════════════════
    let s3 = eval_string(&mut ctx, &format!(r#"
        var results = [];
        try {{
            var fs = require('fs');
            var totalSize = 256;
            var chunkSize = 32;
            var nChunks = totalSize / chunkSize;
            var chunks = [];
            for (var i = 0; i < nChunks; i++) {{
                var b = Buffer.alloc(chunkSize);
                for (var j = 0; j < chunkSize; j++) {{
                    b[j] = (i * chunkSize + j) & 0xff;
                }}
                chunks.push(b);
            }}
            results.push("chunk_count_" + chunks.length);

            // concat (or manual merge)
            var combined;
            if (typeof Buffer.concat === 'function') {{
                combined = Buffer.concat(chunks);
            }} else {{
                combined = Buffer.alloc(totalSize);
                var off = 0;
                for (var i = 0; i < chunks.length; i++) {{
                    var c = chunks[i];
                    for (var j = 0; j < c.length; j++) combined[off + j] = c[j];
                    off += c.length;
                }}
            }}
            results.push("combined_len_" + combined.length);

            // Read byte via Buffer index (works for object buffers)
            function byteAt(buf, i) {{
                if (typeof buf === 'string') return buf.charCodeAt(i) & 0xff;
                if (typeof buf === 'object' && buf !== null) return buf[i];
                return -1;
            }}
            results.push("combined_byte_0_" + byteAt(combined, 0));
            results.push("combined_byte_100_" + byteAt(combined, 100));
            results.push("combined_byte_255_" + byteAt(combined, 255));

            // Encode chunks as hex lines and write textually.
            // (writeFileSync only accepts strings in current implementation.)
            var hexLines = [];
            for (var i = 0; i < chunks.length; i++) {{
                var hex = "";
                for (var j = 0; j < chunks[i].length; j++) {{
                    var v = chunks[i][j];
                    var h = (v & 0xff).toString(16);
                    if (h.length < 2) h = "0" + h;
                    hex += h;
                }}
                hexLines.push(hex);
            }}
            fs.writeFileSync("{big_p}", hexLines.join("\n") + "\n");
            var st = fs.statSync("{big_p}");
            results.push("big_size_positive_" + (st.size > 0));

            // Stream-write each chunk as append (string). Concatenate all hex first,
            // then decode back to a single Buffer to round-trip-verify the pipeline.
            fs.writeFileSync("{chunk_op}", "");
            for (var i = 0; i < hexLines.length; i++) {{
                fs.appendFileSync("{chunk_op}", hexLines[i] + "\n");
            }}
            var st2 = fs.statSync("{chunk_op}");
            results.push("streamed_size_positive_" + (st2.size > 0));

            // Read back the streamed hex, decode to buffer, verify bytes
            var readBack = fs.readFileSync("{chunk_op}", "utf8");
            var readLines = readBack.split(/\n/).filter(function(l) {{ return l.length > 0; }});
            results.push("read_lines_" + readLines.length);

            // Decode hex back to a Buffer (manual: Buffer.from(hex, 'hex') may not be wired)
            var decoded = Buffer.alloc(totalSize);
            var pos = 0;
            for (var i = 0; i < readLines.length; i++) {{
                var line = readLines[i];
                for (var j = 0; j < line.length; j += 2) {{
                    var b = parseInt(line.substring(j, j + 2), 16);
                    decoded[pos++] = b;
                }}
            }}
            results.push("decoded_len_" + decoded.length);
            results.push("decoded_byte_0_" + byteAt(decoded, 0));
            results.push("decoded_byte_100_" + byteAt(decoded, 100));
            results.push("decoded_byte_255_" + byteAt(decoded, 255));

            results.push("CHUNK_DONE");
        }} catch(e) {{
            results.push("CHUNK_ERR:" + (e.message || e));
        }}
        results.join("|");
    "#, big_p = big_p, chunk_op = chunk_op));

    println!("scenario3: {}", s3);
    assert!(s3.contains("CHUNK_DONE"), "chunked Buffer must complete: {}", s3);
    assert!(s3.contains("chunk_count_8"), "8 chunks of 32 bytes: {}", s3);
    assert!(s3.contains("combined_len_256"), "combined length 256: {}", s3);
    assert!(s3.contains("combined_byte_0_0"), "combined[0]=0: {}", s3);
    assert!(s3.contains("combined_byte_100_100"), "combined[100]=100: {}", s3);
    assert!(s3.contains("combined_byte_255_255"), "combined[255]=255: {}", s3);
    assert!(s3.contains("big_size_positive_true"), "big.bin written: {}", s3);
    assert!(s3.contains("streamed_size_positive_true"), "streamed file written: {}", s3);
    assert!(s3.contains("read_lines_8"), "8 hex lines read back: {}", s3);
    assert!(s3.contains("decoded_len_256"), "decoded buffer len 256: {}", s3);
    assert!(s3.contains("decoded_byte_0_0"), "decoded[0]=0: {}", s3);
    assert!(s3.contains("decoded_byte_255_255"), "decoded[255]=255: {}", s3);

    // Rust verify: big.bin should be a non-empty text file with hex lines.
    let big_meta = fs::metadata(&big_bin).expect("big.bin stat");
    assert!(big_meta.len() > 0, "big.bin non-empty from Rust");

    // ═══════════════════════════════════════════════════════════════
    // Scenario 4: Data cleaning — filter invalid, rename fields, coerce types
    // ═══════════════════════════════════════════════════════════════
    let s4 = eval_string(&mut ctx, &format!(r#"
        var results = [];
        try {{
            var fs = require('fs');
            var dirty = JSON.parse(fs.readFileSync("{dirty_p}", "utf8"));
            results.push("dirty_count_" + dirty.length);
            var cleaned = [];
            for (var i = 0; i < dirty.length; i++) {{
                var r = dirty[i];
                // skip if name is empty
                if (!r.name || r.name.length === 0) {{
                    results.push("skip_empty_name_idx_" + i);
                    continue;
                }}
                // skip if amount is not parseable as finite number
                var amt = Number(r.amount);
                if (!isFinite(amt)) {{
                    results.push("skip_invalid_amount_idx_" + i);
                    continue;
                }}
                // rename id -> userId, coerce amount to number, normalize name uppercase
                cleaned.push({{
                    userId: r.id,
                    fullName: r.name.toUpperCase(),
                    amount: amt
                }});
            }}
            results.push("cleaned_count_" + cleaned.length);
            fs.writeFileSync("{clean_op}", JSON.stringify(cleaned));
            var verify = JSON.parse(fs.readFileSync("{clean_op}", "utf8"));
            results.push("verify_len_" + verify.length);
            results.push("verify_first_name_" + verify[0].fullName);
            results.push("verify_first_amt_type_" + typeof verify[0].amount);
            results.push("CLEAN_DONE");
        }} catch(e) {{
            results.push("CLEAN_ERR:" + (e.message || e));
        }}
        results.join("|");
    "#, dirty_p = dirty_p, clean_op = clean_op));

    println!("scenario4: {}", s4);
    assert!(s4.contains("CLEAN_DONE"), "data cleaning must complete: {}", s4);
    assert!(s4.contains("dirty_count_4"), "4 input records: {}", s4);
    assert!(s4.contains("skip_empty_name_idx_1"), "skipped empty name at idx 1: {}", s4);
    assert!(s4.contains("skip_invalid_amount_idx_2"), "skipped NaN at idx 2: {}", s4);
    assert!(s4.contains("cleaned_count_2"), "2 cleaned records: {}", s4);
    assert!(s4.contains("verify_first_name_A"), "first name uppercase A: {}", s4);
    assert!(s4.contains("verify_first_amt_type_number"), "amount is number: {}", s4);

    // ═══════════════════════════════════════════════════════════════
    // Scenario 5: Multi-source join — users + orders by uid
    // ═══════════════════════════════════════════════════════════════
    let s5 = eval_string(&mut ctx, &format!(r#"
        var results = [];
        try {{
            var fs = require('fs');
            var users = JSON.parse(fs.readFileSync("{users_p}", "utf8"));
            var orders = JSON.parse(fs.readFileSync("{orders_p}", "utf8"));
            results.push("users_" + users.length);
            results.push("orders_" + orders.length);
            // build index
            var userIdx = {{}};
            for (var i = 0; i < users.length; i++) {{
                userIdx[users[i].uid] = users[i];
            }}
            // left join orders → users
            var joined = [];
            var unmatched = 0;
            for (var j = 0; j < orders.length; j++) {{
                var o = orders[j];
                var u = userIdx[o.uid];
                if (u) {{
                    joined.push({{
                        oid: o.oid,
                        uid: o.uid,
                        userName: u.name,
                        total: o.total
                    }});
                }} else {{
                    unmatched++;
                    joined.push({{
                        oid: o.oid,
                        uid: o.uid,
                        userName: null,
                        total: o.total
                    }});
                }}
            }}
            results.push("joined_count_" + joined.length);
            results.push("unmatched_" + unmatched);
            // aggregate total by user
            var byUser = {{}};
            for (var k = 0; k < joined.length; k++) {{
                var name = joined[k].userName || "UNKNOWN";
                if (!byUser[name]) byUser[name] = {{ count: 0, total: 0 }};
                byUser[name].count++;
                byUser[name].total += joined[k].total;
            }}
            var agg = Object.keys(byUser).map(function(name) {{
                return {{ name: name, count: byUser[name].count, total: byUser[name].total }};
            }});
            agg.sort(function(a, b) {{ return a.name < b.name ? -1 : 1; }});
            fs.writeFileSync("{join_op}", JSON.stringify({{ joined: joined, summary: agg }}));
            // assertions
            results.push("alice_orders_" + byUser["Alice"].count);
            results.push("alice_total_" + byUser["Alice"].total);
            results.push("bob_total_" + byUser["Bob"].total);
            results.push("unknown_count_" + (byUser["UNKNOWN"] ? byUser["UNKNOWN"].count : 0));
            results.push("JOIN_DONE");
        }} catch(e) {{
            results.push("JOIN_ERR:" + (e.message || e));
        }}
        results.join("|");
    "#, users_p = users_p, orders_p = orders_p, join_op = join_op));

    println!("scenario5: {}", s5);
    assert!(s5.contains("JOIN_DONE"), "join must complete: {}", s5);
    assert!(s5.contains("users_3"), "3 users: {}", s5);
    assert!(s5.contains("orders_4"), "4 orders: {}", s5);
    assert!(s5.contains("joined_count_4"), "4 joined rows: {}", s5);
    assert!(s5.contains("unmatched_1"), "1 unmatched (uid 999): {}", s5);
    assert!(s5.contains("alice_orders_2"), "Alice has 2 orders: {}", s5);
    assert!(s5.contains("alice_total_75"), "Alice total 50+25=75: {}", s5);
    assert!(s5.contains("bob_total_75"), "Bob total 75: {}", s5);
    assert!(s5.contains("unknown_count_1"), "1 unknown: {}", s5);

    // Rust verify
    let joined_str = fs::read_to_string(&join_out).expect("read joined.json");
    assert!(joined_str.contains("\"userName\":\"Alice\""), "joined has Alice: {}", joined_str);
    assert!(joined_str.contains("\"UNKNOWN\"") || joined_str.contains("null"),
        "joined has unknown user: {}", joined_str);

    // ═══════════════════════════════════════════════════════════════
    // Cleanup — best-effort
    // ═══════════════════════════════════════════════════════════════
    // Try fs.rmSync via JS; if not available, fall back to Rust fs call.
    let _ = eval_string(&mut ctx, &format!(r#"
        try {{
            var fs = require('fs');
            if (typeof fs.rmSync === 'function') {{
                fs.rmSync("{d}", {{ recursive: true, force: true }});
                "rm_ok";
            }} else if (typeof fs.rmdirSync === 'function') {{
                // fallback: cannot remove non-empty rmdirSync easily; leave it.
                "no_rm";
            }} else {{
                "no_rm";
            }}
        }} catch(e) {{
            "rm_err:" + (e.message || e);
        }}
    "#, d = d));
    // Always attempt Rust-side cleanup too.
    let _ = fs::remove_dir_all(&dir);

    // Sanity: ensure the PathBuf is dropped cleanly (no leaked handles).
    let _: PathBuf = dir;

    // Leak the JsContext to avoid mozjs GC/TLS destructor crash on drop.
    // mozjs's C++ TLS teardown (mozilla::detail::MutexImpl) segfaults if
    // the Runtime is dropped after JS_ShutDown — intentional skip.
    bun_runtime::shutdown_thread_sm();
}
