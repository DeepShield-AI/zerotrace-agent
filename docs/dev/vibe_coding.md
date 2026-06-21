# Vibe Coding 流程指南

"Vibe Coding" 是一种现代化的编程范式，核心理念是**人类开发者专注于意图（Intent）和审查（Review），而将具体的代码实现（Implementation）交给 AI 辅助工具**。在这种模式下，编程不再是逐行敲击代码的体力活，而是一种与 AI 协作的流畅体验（"Vibe"）。

## 核心原则

1. **意图优先 (Intent First)**: 开发者清晰地描述"想要什么"，而不是"怎么做"。
2. **快速迭代 (Fast Iteration)**: 利用 AI 快速生成原型，通过对话不断修正。
3. **保持流动 (Stay in the Flow)**: 减少查阅文档和搜索语法的时间，保持思维的连贯性。
4. **人工兜底 (Human in the Loop)**: AI 生成代码，人类负责架构决策、代码审查和测试验收。

## 标准 Vibe Coding 流程

### 1. 明确需求 (Define Intent)
在开始 coding 之前，先用自然语言梳理清楚你要完成的任务。
- **Bad**: "写个函数。"
- **Good**: "写一个 Python 脚本，读取当前目录下的所有 CSV 文件，计算由于 'status' 列为 'failed' 的行数，并将结果输出到 summary.txt。"

### 2. 启动上下文 (Context Setup)
确保 AI 了解当前的项目背景。
- 打开相关的文件（Active Files）。
- 使用 `@` 引用关键的类、函数或文档。
- 如果是新功能，简要说明现有的架构约束。

### 3. 生成与交互 (Generate & Interact)
向 AI 发送指令，观察生成的代码。
- **初次生成**: 让 AI 生成代码框架或完整逻辑。
- **微调**: 如果细节不对（如变量命名、错误处理），直接告诉 AI 进行修改 ("把错误处理改成记录日志而不是抛出异常")。

### 4. 审查与应用 (Review & Apply)
不要盲目接受所有代码。
- **Diff View**: 仔细查看 AI 建议的更改对比（Diff）。
- **Logic Check**: 检查业务逻辑是否符合预期。
- **Safety Check**: 确保没有引入安全漏洞或破坏现有功能。
- **Accept**: 确认无误后应用更改。

### 5. 测试与验证 (Test & Verify)
- 让 AI 生成测试用例。
- 运行代码/测试，查看报错信息。
- 如果报错，直接将错误信息复制给 AI，让其分析并修复 ("Fix this error: ...")。

### 6. 提交 (Commit)
- 让 AI 生成 Commit Message ("Generate a commit message for these changes")。
- 提交代码。

## 常见工具
- **Windsurf / Cascade**: 深度集成的 AI 编程助手。
- **Cursor**: 另一款流行的 AI 编辑器。
- **GitHub Copilot**: 代码补全和问答。
