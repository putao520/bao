# sm-audit — mozjs JSAPI capability inventory + drift 自动化(SM-EVOLUTION #30)

每次 mozjs 上游前移,自动回答「SM 新增/删除/改变了哪些 embedding capability?
Bao 用没用?」三脚本 + 一个共享模块,纯 Python3 标准库,零构建。

## 与人工账本的分工(#30 契约)

| 层 | 载体 | 性质 |
|---|---|---|
| symbol 级 FACTS(自动化) | `.claude/sm-audit/inventory-*.json` / `adoption-*.json` | 本工具产出;category_guess/stability_guess 均为启发式 |
| capability 级裁决(人工) | `.claude/sm-capability-ledger.json` | used-native/wrapped/emulated/missing/deliberately-unused 状态由人/Agent 裁决,自动化不裁定 |
| 人类状态账本 | `.plans/spidermonkey-evolution.md` | 每轮执行记录 + 裁决 |

unused-pending-verdict ≠ 该弃用;native-used 低置信标记可能是通用名误报
(裁决前必看 usage_files)。

## 三脚本

```bash
# ① capability extractor:bindgen jsapi.rs(bao-mozjs-sys build out)全量
#    extern fn + mozjs 安全层绑定分类 → inventory-<label>.json
#    自检 fail-closed:阳性对照(JS_NewContext/EnterRealm/ReportOutOfMemory)
#    + extern 块计数;失败=bindgen 形态漂移,退出码 2(降级:以 rust.rs 单源)
python3 scripts/sm-audit/extract_inventory.py            # jsapi.rs 自动发现(最新 build out)
BAO_SM_AUDIT_JSAPI=<path> python3 scripts/sm-audit/extract_inventory.py  # 显式指定

# ③ Bao 使用映射:src/**/*.rs 单遍 tokenize 词边界计数 → 两分类初判
#    native-used / unused-pending-verdict + confidence(high/medium/low)
python3 scripts/sm-audit/usage_map.py                    # 默认取最新 inventory

# ② drift detector:两版 inventory 三分类(added/removed/renamed)
#    + signature_changed / stability_flip + 域级 rollup + Bao 使用标注
python3 scripts/sm-audit/drift.py <old-inventory.json> <new-inventory.json> \
    --out .claude/sm-audit/drift-<old>..<new>.json       # --old-adoption 默认自动发现
```

输入=repo 现状,输出绑定 commit hash + mozjs crate 版本 + jsapi md5
(`inventory.mozjs` 字段),可重复跑。`label` 默认 `<date>-<HEAD short8>`。
当前 drift 基线指针:`.claude/sm-audit/BASELINE`。

## 分类语义

- `surface`: public(C API/JS::)| friend(js::/jsfriendapi)| internal(detail/shadow)
  | glue(jsapi.rs root::glue,mozjs 自产 C++ glue;crate::glue 的 gluebindings.rs
  不在本 inventory)| support(非 SM C++ 支撑,本基线 0 fn)
- `binding`(已绑定面): safe-wrapper(wrappers2=jsapi2_wrappers.in.rs 现行)
  > safe-wrapper(wrappers1-deprecated) > mozjs-layer-reference
  (rust.rs/realm.rs/context.rs/... 引用)> raw-only(仅 unsafe 原始可达)
- `category_guess`: 对齐 sm-capability-ledger 13 域,启发式词规则
  (sm_audit_common.CATEGORY_RULES),兜底 core-value/object-surface

## daily-ops 集成点(建议;不改 daily-ops 本体)

mozjs 升级波(§9 长任务协议)编译绿后、patch replay 前:

1. `extract_inventory.py` 生成新 inventory(新 label=升级后 HEAD);
2. `drift.py BASELINE 指向的旧 inventory <新 inventory> --out ...` 产 drift 报告;
3. 消费(不得只编译迁移——#30 DoD):
   - removed/renamed/signature_changed 且 `bao_usage_count>0` → 阻断候选,
     按 BCE 纪律处理调用点;
   - removed 项对照 CLAUDE.md「mozjs fork BAO patch 清单」——被上游取代的
     patch 移除,不盲目重放(#30 patch replay 联动);
   - added 按 adoption policy 分类:无关→deliberately-unused 记因;
     相关但 unstable→blocked;相关且 stable→开/链接 SM-EVOLUTION issue;
     critical correctness/security 可当波内吸收;
   - 域级 rollup 追加 `.plans/spidermonkey-evolution.md` §8(from/to ref +
     drift summary + 裁决结果);
4. 新 inventory 路径写入 `BASELINE`(前移基线);
5. `usage_map.py` 重跑得新 adoption(新增 symbol 的初判)。

日常(非升级波)不需要跑;cap ledger `last_audited` 更新轮可选择性重跑 usage_map
刷新计数。
