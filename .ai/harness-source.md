# ハーネス更新記録

- 更新日: 2026-10-28 JST
- 原本: /Users/takashi/mydev/ai-development-harness-template
- 参照コミット: `930a9313482155ab4af65c71f3a8d9626c83813a`
- 原本の新規・既存対応と乖離レベルの節を追加し、task更新の競合規則のみ修正。既存本文・固有契約・task履歴は維持。原本READMEと保守taskはコピーしていない。

## 2026-11-01 最新ハーネスの差分統合

- 取得元: https://github.com/buraiha/ai-development-harness-template
- 適用元コミット: `20d937cef7cc865c4ae7fd388f1317025594ce77`（GitHub mainと照合済み）。
- 固有契約・要件正本・既存task本文・既存の作業ツリー変更を維持し、共通ルールを差分統合。原本の製品README・保守taskはコピーしていない。
- リリース専用チャットの指定は既存の明示指示を維持し、未指定の場合は未指定と記録。手動リリースは対象外。
- アプリコード・依存関係・実行設定は変更せず、今回のハーネス文書のコミット・pushは2026-11-01にユーザー承認済み。
- 作業記録: `.ai/tasks/0091-update-shared-harness.md`（パスはリポジトリルート基準）。

## 2026-11-02 試験結果の保存・確認ルール

- 適用元: `buraiha/ai-development-harness-template` の `509cf9f602348f21ace80108833547251a5a6803`。
- 今回の試験結果ルールのみ差分統合。既存本文・固有契約・既存変更・結果の保存場所を保持。
- 記録: `.ai/tasks/0092-store-task-test-results.md`。今回のコミット・pushは未実施。

## 今回の適用状態

- 原本: https://github.com/buraiha/ai-development-harness-template
- 適用元コミット: `6488c680ffa01af6e92b643dcd542be44759437d`（共通ルール・版タグ運用を反映）。
- 統合: 固有契約・既存変更・過去task・試験結果を保持して本文へ差分統合。原本の製品README・保守taskはコピーしない。
- 製品版の正本: `VERSION`。変更履歴: `CHANGELOG.md`。ユーザー指定の開始値を設定済み。
- 作業記録: `.ai/tasks/0093-integrate-versioning-harness.md` とその後の版管理・チェンジログ・タグ運用task（パスはリポジトリルート基準）。
- 今回のコミット・pushはユーザー承認済み。製品リリース・版タグ作成は行わない。

## 2026-11-02 統合内容の整合修正

- 同じ適用元 `6488c680ffa01af6e92b643dcd542be44759437d` のtask運用を補完し、Status定義・重複・別プロジェクトの記述混入を修正。既存task・試験証跡と固有契約を保持。
- 作業記録: `.ai/tasks/0098-fix-harness-integration.md`。この修正のコミット・pushは未実施。
