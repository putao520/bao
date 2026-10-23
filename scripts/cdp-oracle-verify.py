#!/usr/bin/env python3
# @tool REQ-CDP-010
# CDP oracle 双载具逐字段同形 verify 探针(Chrome 155 headless=new oracle vs bao browser --cdp-port)。
# 连接两端 CDP over WebSocket,发同一组方法,逐字段 diff,产出 JSON 对照报告。
# 用法: python3 cdp-oracle-verify.py --oracle-port 9333 --bao-port 9334 [--out REPORT.json]
import argparse
import json
import subprocess
import sys
import time
import urllib.request

try:
    import websocket  # websocket-client
except ImportError:
    print("ERROR: python3-websocket-client required (apt install python3-websocket-client)", file=sys.stderr)
    sys.exit(2)


def http_json(port, path):
    url = f"http://127.0.0.1:{port}{path}"
    with urllib.request.urlopen(url, timeout=10) as r:
        return json.loads(r.read().decode("utf-8"))


class CDP:
    def __init__(self, port):
        self.port = port
        targets = http_json(port, "/json/list")
        page = next((t for t in targets if t.get("type") == "page"), None)
        if page is None:
            raise RuntimeError(f"no page target on port {port}: {targets}")
        self.ws = websocket.create_connection(page["webSocketDebuggerUrl"], timeout=30)
        self._id = 0

    def cmd(self, method, params=None):
        self._id += 1
        mid = self._id
        self.ws.send(json.dumps({"id": mid, "method": method, "params": params or {}}))
        deadline = time.time() + 30
        while time.time() < deadline:
            msg = json.loads(self.ws.recv())
            if msg.get("id") == mid:
                return msg
        raise RuntimeError(f"timeout waiting for {method}")

    def close(self):
        try:
            self.ws.close()
        except Exception:
            pass


# 逐字段同形判定的字段集合(REQ-CDP-010 台账 GOLDEN C 基线焦点)。
# 路径以 CDP 响应 result payload 为根(不含外层 envelope)。
COMPARE_FIELDS = {
    "Runtime.evaluate": ["result.type", "result.value", "result.description", "exceptionDetails.text"],
    "Page.getFrameTree": ["frameTree.frame.id", "frameTree.frame.url", "frameTree.frame.loaderId"],
}


def get_nested(obj, dotted):
    cur = obj
    for part in dotted.split("."):
        if isinstance(cur, dict) and part in cur:
            cur = cur[part]
        else:
            return None, False
    return cur, True


def compare_field(oracle_val, bao_val):
    # JSON 同形:类型一致 + 值一致(浮点宽容)
    if isinstance(oracle_val, float) and isinstance(bao_val, (int, float)):
        return abs(float(oracle_val) - float(bao_val)) < 1e-9
    return oracle_val == bao_val and type(oracle_val) is type(bao_val)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--oracle-port", type=int, default=9333)
    ap.add_argument("--bao-port", type=int, default=9334)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    oracle = CDP(args.oracle_port)
    bao = CDP(args.bao_port)
    report = {"oracle_port": args.oracle_port, "bao_port": args.bao_port, "fields": []}

    # 冒烟 evaluate:两端各求值同一表达式
    expr = "({a:1,b:'x',c:[1,2,3],d:{n:null}})"
    ore = oracle.cmd("Runtime.evaluate", {"expression": expr, "returnByValue": True}).get("result", {})
    bre = bao.cmd("Runtime.evaluate", {"expression": expr, "returnByValue": True}).get("result", {})

    for field in COMPARE_FIELDS["Runtime.evaluate"]:
        ov, ok_o = get_nested(ore, field)
        bv, ok_b = get_nested(bre, field)
        # 双方都缺字段 = 同形(该方法无此返回面);一方有一方无 = 偏差
        if not ok_o and not ok_b:
            match = True
            ov = bv = "<both-absent>"
        else:
            match = ok_o and ok_b and compare_field(ov, bv)
        report["fields"].append({
            "method": "Runtime.evaluate", "field": field,
            "oracle": ov if ok_o else "<absent>", "bao": bv if ok_b else "<absent>",
            "match": bool(match),
        })

    # 结构骨架同形:result 键集合
    def keys_of(resp):
        r = resp.get("result", {})
        res = r.get("result", {})
        return sorted(res.keys()) if isinstance(res, dict) else None
    report["fields"].append({
        "method": "Runtime.evaluate", "field": "result.keys",
        "oracle": keys_of(ore), "bao": keys_of(bre),
        "match": keys_of(ore) == keys_of(bre),
    })

    # Page.getFrameTree loaderId 存在性(台账停报②:无 per-load 持久身份,只判存在与同形)
    ore_ft = oracle.cmd("Page.getFrameTree").get("result", {})
    bre_ft = bao.cmd("Page.getFrameTree").get("result", {})
    for field in COMPARE_FIELDS["Page.getFrameTree"]:
        ov, ok_o = get_nested(ore_ft, field)
        bv, ok_b = get_nested(bre_ft, field)
        if not ok_o and not ok_b:
            match = True
            ov = bv = "<both-absent>"
        else:
            # loaderId/ids 是身份字段,两端各自唯一即可(不逐字相等,只判存在+同型)
            match = ok_o and ok_b and type(ov) is type(bv)
        report["fields"].append({
            "method": "Page.getFrameTree", "field": field,
            "oracle": ov if ok_o else "<absent>", "bao": bv if ok_b else "<absent>",
            "match": bool(match),
        })

    oracle.close()
    bao.close()

    n_match = sum(1 for f in report["fields"] if f["match"])
    n_total = len(report["fields"])
    report["summary"] = {"match": n_match, "total": n_total, "all_match": n_match == n_total}

    out = json.dumps(report, indent=2, ensure_ascii=False)
    if args.out:
        with open(args.out, "w", encoding="utf-8") as f:
            f.write(out)
    print(out)
    sys.exit(0 if report["summary"]["all_match"] else 1)


if __name__ == "__main__":
    main()
