# ハーネス統合時の不整合修正

- Status: completed
- Depends on: none

## 目的と要求

ユーザーの「修正して」に基づき、調査済みの4点（task運用の反映漏れ、Status定義の不統一、ガイドの重複、別プロジェクトの記述混入）を修正する。

## 変更対象

- `.ai/README.md`
- `.ai/tasks/README.md`
- `.ai/guides/03-implementation.md`
- `.ai/guides/04-testing.md`
- `.ai/harness-source.md`
- 本taskと `.ai/tasks/0098/test-results.md`

## 制約・対象外

適用元は確認済みの配布元コミット `6488c680ffa01af6e92b643dcd542be44759437d`。プロジェクト固有契約、既存taskと試験証跡は維持する。アプリ、版番号、配布元リポジトリは変更しない。コミット・push・リリースは行わない。

## 受け入れ条件と検証

- task運用は適用元と整合し、旧記録の保持規則も維持する。
- implementedをStatus一覧・説明へ統合する。
- 指摘された同一文章の重複とfishing-logの記述を解消する。
- 配布元との差分、相対リンク、文書の空白、変更範囲を確認する。

## 試験結果

[試験結果](0098/test-results.md)

## 実施結果

4点の修正と文書検証を完了。配布元のtask運用を統合し、固有の既存記録保持規則を維持した。
