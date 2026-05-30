// test_event_loop_order.js — REQ-ENG-004: 宏任务/微任务执行顺序验证
// SPEC 验收标准: C4 宏任务和微任务执行顺序正确

var passed = 0;
var failed = 0;

function assert(condition, msg) {
  if (condition) { passed++; }
  else { failed++; console.log("FAIL: " + msg); }
}

function assertEqual(actual, expected, msg) {
  if (actual === expected) { passed++; }
  else { failed++; console.log("FAIL: " + msg + " — expected " + JSON.stringify(expected) + " got " + JSON.stringify(actual)); }
}

// === TEST EL-001: Promise.then (微任务) 先于 setTimeout (宏任务) 执行 ===
(function testMicroBeforeMacro() {
  var order = [];

  setTimeout(function() { order.push("timeout"); }, 0);
  Promise.resolve().then(function() { order.push("micro"); });

  setTimeout(function() {
    assertEqual(order[0], "micro", "EL-001: microtask (Promise.then) runs before macrotask (setTimeout)");
    assertEqual(order[1], "timeout", "EL-001: macrotask (setTimeout) runs after microtask");
    assertEqual(order.length, 2, "EL-001: exactly 2 items");

    // === TEST EL-002: 嵌套微任务链正确顺序 ===
    (function testNestedMicrotasks() {
      var order2 = [];

      Promise.resolve().then(function() {
        order2.push("a");
        return Promise.resolve().then(function() {
          order2.push("b");
          return Promise.resolve().then(function() {
            order2.push("c");
          });
        });
      });

      setTimeout(function() {
        assertEqual(order2.length, 3, "EL-002: nested microtask chain length");
        assertEqual(order2[0], "a", "EL-002: first microtask");
        assertEqual(order2[1], "b", "EL-002: second microtask");
        assertEqual(order2[2], "c", "EL-002: third microtask");

        // === TEST EL-003: queueMicrotask 与 Promise.then 交替执行 ===
        (function testQueueMicrotaskOrder() {
          var order3 = [];

          queueMicrotask(function() { order3.push("qm1"); });
          Promise.resolve().then(function() { order3.push("p1"); });
          queueMicrotask(function() { order3.push("qm2"); });
          Promise.resolve().then(function() { order3.push("p2"); });

          setTimeout(function() {
            assertEqual(order3.length, 4, "EL-003: queueMicrotask + Promise.then all executed");
            // 微任务按注册顺序执行
            assertEqual(order3[0], "qm1", "EL-003: first queueMicrotask");
            assertEqual(order3[1], "p1", "EL-003: first Promise.then");
            assertEqual(order3[2], "qm2", "EL-003: second queueMicrotask");
            assertEqual(order3[3], "p2", "EL-003: second Promise.then");

            // === TEST EL-004: setTimeout 0 vs setTimeout 100 顺序 ===
            (function testTimerOrder() {
              var order4 = [];

              setTimeout(function() { order4.push("t100"); }, 100);
              setTimeout(function() { order4.push("t50"); }, 50);
              setTimeout(function() { order4.push("t0"); }, 0);

              setTimeout(function() {
                assertEqual(order4[0], "t0", "EL-004: setTimeout 0 fires first");
                assertEqual(order4[1], "t50", "EL-004: setTimeout 50 fires second");
                assertEqual(order4[2], "t100", "EL-004: setTimeout 100 fires third");

                // === TEST EL-005: setInterval 执行次数验证 ===
                (function testSetInterval() {
                  var count = 0;
                  var id = setInterval(function() {
                    count++;
                    if (count >= 3) {
                      clearInterval(id);
                      assert(count >= 3, "EL-005: setInterval fired at least 3 times");
                      finishTests();
                    }
                  }, 10);
                })();
              }, 200);
            })();
          }, 50);
        })();
      }, 50);
    })();
  }, 50);
})();

var finishCalled = false;
function finishTests() {
  if (finishCalled) return;
  finishCalled = true;

  console.log("\n========== Event Loop Order Test ==========");
  console.log("PASSED: " + passed);
  console.log("FAILED: " + failed);
  console.log("============================================");
  console.log(failed === 0 ? "RESULT: ALL PASS" : "RESULT: HAS FAILURES");
}
