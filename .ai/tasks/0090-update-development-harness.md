# 共通ハーネスの差分統合

- Status: completed
- Depends on: none

## 目的と範囲

ユーザー依頼に従いAGENTS.md、.ai/common.md、.ai/README.md、.ai/project.md、.ai/guides/、.ai/tasks/README.md、.ai/harness-source.md、本taskへ新規・既存対応と乖離レベルを統合する。

## 制約・検証

既存本文・固有契約・task履歴を保持し、競合箇所のみ修正する。アプリ・データ・環境は変更しない。コミット・pushは行わない。差分・参照先・git diff --checkを確認する。

## 実施結果

- 共通規則を追記し、task作成・更新の競合箇所を調整。固有契約と既存task本文は保持した。OGIMSのproject.mdは既存構造を保ちプレースホルダーを確認済みの構成・未確認事項で補完した。
- 文書差分・追加リンク・git diff --checkを確認。アプリ動作試験は文書変更のため対象外。コミット・pushは未実施。

## 追加依頼（2026-10-28）

ユーザーの追加指示に従い、.gitignoreのmacOSメタデータ除外を確認・補完し、今回のハーネス変更と作業記録をコミットする。先の「コミットしない」はこの指示で更新する。メタデータは削除せず無視し、アプリ・実データは変更しない。
