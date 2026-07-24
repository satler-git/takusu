# スキルの形式と活用

`skills_list`、`skills_read`、`skills_propose_add`、`skills_propose_edit` は Deferred ツールである。
使う前に `tool_search` で発見する。

スキルは、エージェントの振る舞いをカスタマイズするマークダウン形式の指示書である。
TOML front matter と本文で構成される。

## スキルのマークダウン形式

```markdown
+++
name = "weekly-review"
description = "Run a weekly review to clean up stale tasks and plan the next week."
+++

Run the weekly review by:
1. ...
```

`name` と `description` は front matter に含める。
本文には、LLM が実行すべき手順を自由に書く。

## ツール

- `skills_list`：利用可能なスキルを一覧する
- `skills_read`：slug を指定して本文を含む詳細を読む
- `skills_propose_add`：新しいスキルを追加する提案を生成する
- `skills_propose_edit`：既存スキルを編集する提案を生成する

built-in スキルは `skills_propose_edit` で編集できない。
新規追加するときは、slug が既存のスキルと重複しないようにする。
