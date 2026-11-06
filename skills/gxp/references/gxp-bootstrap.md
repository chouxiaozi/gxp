# gxp 自举约束 + Edit 后引用检查清单

> 来源：AGENTS.md §6.5 自举强制约束 + §6.5.1 Edit 后引用检查清单
> 适配：原约束绑定 gxp 仓库自身；本文档改写为通用表述，分发到所有项目

## 1. 自举约束（gxp 仓库内强制，其他项目建议）

**gxp 仓库内**（cwd basename = `gxp`）：搜索/验证/解释三阶段必须用 gxp 而非 Grep 工具，违反等同不变式违反。理由：gxp 是本仓库产物，也是工作工具；改代码前搜索、改代码后验证、重构后对账三个阶段都必须自举。

**其他项目**（gxp 已安装）：强烈建议用 gxp 替代 Grep 工具，因为：
1. **可信度对账基础**：gxp BARE 模式与 `grep -E` 行为一致，结果可对账
2. **应用友好**：DSL 提供字段谓词/Top-K/管道组合，比 Grep 工具一次只能拿 raw 行更高效
3. **`--verify` 暴力对账**：可验证查询正确性（与优化路径结果 diff，0 FAIL 才可信）

## 2. 三类搜索场景

| 场景 | 用什么 | 示例 |
|---|---|---|
| 找字面量/函数调用 | gxp BARE 模式 | `gxp 'fn name' src/` |
| 找结构重复 | gxp ast 查询 | `gxp 'ast(rs, "if $C { $A } else { $B }")' src/` |
| 验证 Edit 后引用 | gxp --verify | `gxp '"new_name"' src/ --verify` |

**为什么不用 Grep 工具**：
- Grep 工具找不到结构（无法表达 `if $C { $A } else { $B }` 这种 AST 模式）
- Grep 工具无 `--verify` 对账，结果可信度无保证
- LLM 默认走 Grep 是已记录偏移（规则文档无法根治，但自举让违反显式化、可追溯）

## 3. 搜索结果可信度判断

| 查询类型 | 可信度判据 |
|---|---|
| BARE 模式 | 结果必须与 `grep -E` 一致（不一致即 bug） |
| ast() 查询 | 必须 `--verify` 0 FAIL 才可信 |
| DSL 字段谓词 | `--verify` 0 FAIL 才可信（L1 ⊇ L2 不变式） |
| 流式路径（>512MB） | 不支持 trace / percentiles（需 O(N) 内存）——用 `context N` 替代 |

## 4. Edit 后引用检查清单

每次 Edit 后立即跑对应自举查询，确认引用点齐全。

> 路径用被改项目自身源码目录（如 `src/`、`lib/`、`app/`），下例以 `src/` 示意。

| Edit 类型 | 自举命令（在源码目录上跑） | 期望覆盖点 |
|---|---|---|
| 新增 enum 变体 / 派生类 | `gxp '"EnumName::Foo"' src/ --verify` | ≥3 处：定义 + match 分派 + 测试断言（最佳实践 ≥6 处含文档/调用点） |
| 新增 struct 字段 | `gxp '"字段名"' src/ --verify` | ≥3 处：struct 定义 + 构造点 + JSON/序列化输出 |
| 新增公开 `fn` | `gxp 'fn name' src/ --verify` | ≥1 调用点（不在 `#[cfg(test)]` 块内） |
| 改函数签名 | `gxp 'fn name' src/ --verify` + 跑项目构建 | 所有调用点 0 编译错误 |

**触发时机**：每次 Edit 后、commit 前必跑。

**判通过**：`--verify` 0 FAIL 且引用点数 ≥ 期望；不达 → 立即补引用点或修复查询。

## 5. 重构后对账

拆分/搬迁后用 gxp 对比前后完整性：

```bash
# 函数清单完整性
gxp 'fn test_' tests/ > before.txt
# ... 重构 ...
gxp 'fn test_' tests/ > after.txt
diff before.txt after.txt            # 应无差异

# 调用点全量验证
gxp 'helper_name' src/ --verify     # 0 FAIL
```

## 6. 不变式校验（必须 100% 通过）

| 不变式 | 验证方式 |
|---|---|
| `L1(粗筛) ⊇ L2(精判)` | `--verify` 暴力对账（每个 L1+L2 查询） |
| `gxp 'pattern' file` ≡ `grep -E 'pattern' file`（BARE 模式） | 每个改动跑 `tests/mode_bare_ere_compat.rs` |
| `--verify` 与精确结果 100% 一致 | `run_verify` 行集合 diff |

## 7. 常见偏移

- **LLM 默认用 Grep 工具而非 gxp 自举**：已重犯多次。规则文档无法根治，但强制约束让违反显式化、可追溯。
- **修复策略**：搜索/验证/解释阶段看到自己想用 Grep 工具时，先停一下，问"这个查询 gxp 能做吗？"——BARE 模式覆盖所有 `grep -E` 场景，ast() 覆盖所有结构场景。

## 8. 流式路径限制

文件 > 512MB 走 BufReader 流式扫描，限制：
- 不支持 `trace`（跨行边界判定需"看到下一行"）——用 `context N` 替代
- 不支持 `percentiles`（O(N) 内存缓冲）——先 `sample` 抽样再处理
- 其他终止 stage（count/group/stats）仍可用
