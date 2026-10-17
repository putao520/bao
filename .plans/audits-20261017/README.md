# 2026-10-17 四维审计与清偿台账(会话交付存档)

用户令:基于时序概念与成熟库包概念,核实全项目的可归并/性能可优化/编译加速/产出大小四维升级方案。

## 报告清单
| 报告 | 内容 | 落地状态 |
|---|---|---|
| consolidate-perf-scan.md | 可归并+性能审计(A1/A2/A3+B1/B2/B3) | A1(属性遍历 ~400 行)+A2(percent_decode 规范)+B1/B2/B3 已实施提交 |
| compile-audit-report.md | 编译加速(sccache/incremental/target 树) | ROI-1 已落地(test-ci incremental=false);ROI-3 纪律落地(bd/ntd+memory);ROI-4 已落地(sccache 200G);ROI-2 ②套娃层 19G 已删/③/tmp scratch 已清;①双插桩树合并待用户裁决 |
| thin-lib-audit-report.md | 薄库替换初审(rustix 镜头) | 部分被 functional-replacement 修正 |
| functional-replacement-report.md | 功能级库替换全谱(9 项判定) | #67 第一批已提交;#69 fs.watch→notify 在飞;#67 addons(termios/inet_pton)在树待提交 |
| dedup-versions-report.md | 同库多版本二进制重复审计+campaign | #65 已实施(94/44→79/38;42/43 族 EXT/VENDOR 锚定);tungstenite 0.30 已升级 |
| size-audit-report.md | 产出大小矩阵(strip 135MiB/构成) | P0/P1/P2 方案待用户裁决 |

## Goal C 天花板核算(同日,主会话直读)
排除杠杆上限 +3.2pp:webcore_faces.rs 99 行+bao_native_stubs 200 行+windows 平台面(306 attr);
三档位 ceiling 74.23%/75.33%/77.41%;98% 字面值需 30-36K 新覆盖行=长 campaign,分母治理仅使目标诚实。
已登记缺口六文件(web_api 479/engine_props 232/addrinfo 84/uloop 92/web_streams 44/poll 33)=缺口 2.5%。
处置:C10 波(排除注解落地+缺口补测)执行"推到结构上限"授权面。

## 提交映射
- be079777a 薄库第一批 / 2f35e5bf6 #66 清偿(6 对重复 3 治 3 legitimate+bun_sm DRY)
- (树上待统一提交)#67 addons 三文件 / C10 产出 / #69 worktree 合并
