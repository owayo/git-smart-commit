# 連携

git-sc は、ほかの道具から呼ばれる前提で作っています。エージェントのターンが終わるたびにコミットする Claude Code の Stop フックの設定は [README](../README.ja.md#claude-code-との連携) にあります。このページでは、それ以外に受け取れる情報と、組み込める先を説明します。

## エージェントコンテキスト（claw-hooks 連携）

git-sc を [claw-hooks](https://github.com/owayo/claw-hooks) と併用すると、コーディングエージェントが何をしていたかという情報（コンテキスト）が、環境変数 `CLAW_HOOKS_AGENT_MESSAGE` で自動的に渡されます。git-sc はこれを AI へのプロンプトに含めるので、生成されるコミットメッセージには、差分からは読み取れない変更の意図まで反映されます。

**claw-hooks** は、Claude Code のフックのライフサイクルを管理する連携ツールです。Stop フックが発火すると、エージェントが直前に行った作業の要約を `CLAW_HOOKS_AGENT_MESSAGE` にセットしてから git-sc を呼び出します。

```bash
# claw-hooks の stop hook が自動的にセットします
# CLAW_HOOKS_AGENT_MESSAGE="認証モジュールをJWTトークン方式にリファクタリング"
git-sc -a -y -q
```

この環境変数がセットされていると、プロンプトに `<agent-context>` ブロックが加わり、開発者の意図を優先するよう AI に指示します。これは通常のコミット生成に加えて、`--amend`、`--reword`、`--squash`、`--generate-for` でも適用されます。

## VS Code 拡張機能

**[Git-SC (Smart Commit)](https://marketplace.visualstudio.com/items?itemName=owayo.vscode-git-smart-commit)**: VS Code マーケットプレイスで公開中
