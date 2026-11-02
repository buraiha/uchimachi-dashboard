# AIハーネスの整合確認と修正

- Status: completed
- Depends on: none

## 目的と要求

2026-11-02のユーザー依頼に基づき、調査済みの反映漏れ、Status不統一、重複、別プロジェクト記述混入を確認し、該当箇所を修正する。比較元は `6488c680ffa01af6e92b643dcd542be44759437d`。

## 変更対象

必要な場合のみ `.ai/README.md`、`.ai/tasks/README.md`、`.ai/guides/03-implementation.md`、`.ai/guides/04-testing.md`、導入記録（Madoportは `docs/harness-source.md`、他は `.ai/harness-source.md`）。本taskと試験結果。該当する不整合がない場合は確認結果だけを記録する。

## 対象外・制約

アプリ、要件・仕様、プロジェクト固有契約、既存task・試験証跡は保持。既存未コミット変更を保護する。古い版の一括更新、コミット・push・リリース・採番は実施しない。Uchimachiは先行task0098の差分を保持して再確認する。

## 受け入れ条件・検証

指摘箇所の解消、文書の相対リンクと空白、修正差分の範囲を確認。作業開始時の未コミットファイルのハッシュを比較して保護を確認する。文書変更のみのためアプリ試験は対象外。

## 作業前の状態

````text
 M .ai/README.md
 M .ai/guides/03-implementation.md
 M .ai/guides/04-testing.md
 M .ai/harness-source.md
 M .ai/tasks/README.md
?? .ai/tasks/0098-fix-harness-integration.md
?? .ai/tasks/0098/
````

## 試験結果

[試験結果](0099/test-results.md)

## 実施結果

先行task0098の修正内容・試験結果を再確認し、既存差分をそのまま保持。新taskには確認結果のみ記録。 必要な文書検証を完了。詳細は試験結果を参照。
