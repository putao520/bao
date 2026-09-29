# Example 07 — 执行超时(ExecutionControl)

`evaluate_js[_web]_with_timeout` 给任意脚本一个引擎级硬超时:经 SpiderMonkey 中断回调
实现,死循环也会被掐断并返回稳定的超时语义(不是外层轮询杀线程)。

## 运行

```bash
cargo run
```

## 核心 API 调用

```rust
let v = page.evaluate_js_with_timeout("6 * 7", Some(Duration::from_secs(5)))?;   // Ok("42")
let err = page.evaluate_js_with_timeout("while (true) {}", Some(Duration::from_millis(500)))
    .expect_err("runaway");   // "Script terminated: deadline exceeded (timeout)"
```

## 关键点

- 三个入口同受控:`evaluate_js_with_timeout`(Node Realm)/ `evaluate_js_web_with_timeout`(Page Realm);CLI 的 `--timeout` 是同一层的前缀包装
- 超时后引擎保持健康:下一次求值照常工作(示例断言)
- 超时不是猜测的挂钟轮询——是 SM interrupt 机制,runaway 脚本在下一个中断检查点被终止
