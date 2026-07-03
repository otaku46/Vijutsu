# Vijutsu 要件定義書

**文書版:** v0.2 Draft
**対象:** Jujutsu向けTUI
**仮称:** Vijutsu
**一行定義:** Jujutsuのchange graphとoperation graphを、差分付きで観測・編集する視覚的履歴エディタ

---

## 0. v0.2における主要変更

v0.1から、以下を追加・変更した。

1. CLI出力の機械可読化と設定隔離を必須要件化
2. JSONLベースのparse protocolを定義
3. 外部プロセスによるoperation変更の検知を追加
4. 複数operation headを安全に観測する手順を追加
5. `OperationDelta`を、二時点のスナップショット比較から計算する方式へ変更
6. Inspectモードにsnapshot鮮度表示を追加
7. `redo`などのバージョン差に対する縮退動作を定義
8. 生のrevset入力欄とoperation limitをMVPへ追加
9. グラフ移動とペイン移動のキー割当を分離
10. Phase 0にconfig汚染、二端末並行操作、operation head分岐試験を追加

---

# 1. 背景

Jujutsuには、二つの履歴構造が存在する。

## 1.1 第1層：Change graph

ファイル内容と変更の派生関係を表すグラフ。

```text
@ C
│
○ B
│
○ A
```

各ノードはcommitであると同時に、rewriteをまたいで追跡されるchangeとしての同一性を持つ。

working copyの編集によってcommit IDが変化しても、同じ論理的変更を編集している限り、change IDは維持される。

## 1.2 第2層：Operation graph

change graph、bookmark、workspaceなど、リポジトリ全体の状態がどのように変形されたかを表すグラフ。

```text
o0 ── o1:new ── o2:squash ── o3:describe
│       │            │              │
G0      G1           G2             G3
```

各operationは、その時点のrepository viewへ対応する。

したがって、Jujutsuの状態構造は次のように表せる。

```text
operation graph
    ↓ 各operationがviewを持つ
repository view
    ↓
change graph
    ↓
files / diff
```

operation graphは単純な線形undo stackではない。並行するJujutsu操作によって、複数のoperation headや分岐が発生し得る。

## 1.3 Vijutsuが解決する問題

CLIのみを使用する場合、ユーザーは次の対応を頭の中で再構成する必要がある。

* 今見ているoperationはどれか
* そのoperation時点のchange graphはどの形か
* 同じchangeがどのcommitへrewriteされたか
* squashやrebaseによってどの差分が移動したか
* restore、revert、abandonが二階のグラフへ何を行うか
* どの状態が現在も回復可能か

Vijutsuは、この二階構造を一画面で結び付ける。

---

# 2. プロダクト目的

Vijutsuの目的は、Jujutsuコマンドをメニューから呼び出せるようにすることではない。

目的は次の四点である。

1. 現在のchange graphを視覚的に理解できる
2. 各operation時点のchange graphを遡って観測できる
3. operation間で何が変化したかを比較できる
4. 変更操作の実行前に、二階のグラフへの効果を理解できる

中心価値は、次の問いに即座に答えられることである。

* 現在どのchangeを編集しているか
* このファイル差分はどのchangeに属するか
* 選択したoperation時点では、change graphがどの形だったか
* squashやrebaseによって何がrewriteされたか
* restore、revert、abandonで何が残るか
* 現在表示しているworking-copy diffは、ファイルシステムの最新状態か
* 別ターミナルやエージェントによって状態が更新されたか
* operation historyからまだ回復可能か

---

# 3. 設計原則

## 3.1 Jujutsuを唯一の正とする

Vijutsu内部で独自にリポジトリ状態を確定しない。

```text
ユーザー操作
    ↓
実行予定コマンドの生成
    ↓
二階のグラフ変化をプレビュー
    ↓
jjコマンド実行
    ↓
実状態の再取得
    ↓
画面更新
```

Vijutsuの内部モデルは、Jujutsuから再取得した情報によってのみ確定する。

## 3.2 CLIを初期バックエンドとする

MVPではJujutsu CLIをバックエンドとして使用する。

ただし、Jujutsu公式FAQが述べているとおり、`jj-lib`とCLIのどちらも安定APIではない。したがって、CLI利用は「CLIが安定しているから」ではなく、カスタムbackendを含む利用者の実際の`jj`バイナリをsource of truthにできるために選択する。

将来、次の条件を満たした場合は`jj-lib` backendを追加できる。

* API変更追従コストが許容できる
* カスタムbackend非対応を明示できる
* CLI backendと同じ内部IRを生成できる

## 3.3 InspectとMutateを分離する

Vijutsuは二つの実行モードを区別する。

### Inspect

* repository viewを変更しない
* working copyをsnapshotしない
* operation headを統合しない
* 過去operationを明示的なIDで参照する

### Mutate

* working copyのsnapshot
* change graphのrewrite
* bookmark移動
* operationの追加・復元・反転・破棄

`jj op log`を含む通常のJujutsuコマンドは、working copyのsnapshotやdivergent operationの統合を行い得る。現在のCLIでは、特定operationを`--at-op`で読み込むとworking copyは無視され、`--at-op=@`はdivergent operationを自動統合しない。

したがってInspectモードでは、暗黙の最新状態を参照する通常実行を避ける。

## 3.4 二階プレビュー

変更操作の実行前に、最低限次を表示する。

```text
Change graph:       変わる / 変わらない
Operation graph:    ノード追加 / 除去 / 再接続
Working copy:       書き換わる / 変わらない
Bookmarks:          移動する / 変わらない
Remote tracking:    変わる / 変わらない / 不明
Recoverability:     維持 / 低下 / GC後に喪失し得る
```

## 3.5 選択状態の復元

refresh後の選択対象は、次の順序で復元する。

### Operationペイン

1. 同じoperation ID
2. 選択operationの子孫
3. 選択operationの親
4. 直前の画面位置
5. 明示的に選ばれたcurrent head
6. 安全なデフォルト

### Changeペイン

1. 同じchange IDかつ同じcommit ID
2. 同じchange ID
3. 同じcommit ID
4. rewrite predecessor / successor
5. 親change
6. 直前の画面位置
7. working-copy change
8. 安全なデフォルト

divergent changeではchange IDだけで一意に決定せず、候補を表示する。

## 3.6 内部ストレージを直接変更しない

Vijutsuは`.jj`内部のobjectやoperation headを直接書き換えない。

ファイル監視目的で内部パスを参照する場合も、変更検知のシグナルとしてのみ使う。リポジトリ状態の取得と変更は常に`jj`経由で行う。

---

# 4. 対象ユーザー

## 4.1 主要ユーザー

* Jujutsuを学習中の開発者
* CLIだけではgraph rewriteを追いにくい利用者
* patch stackを頻繁に編集する利用者
* 複数workspaceを使用する利用者
* 別ターミナルで複数の`jj`コマンドを実行する利用者
* AIエージェントを並行稼働させる利用者

## 4.2 想定する最低限の知識

* working copy
* change
* commit
* bookmark
* parent / descendant
* diff

operation log固有の意味は、Vijutsu上で学習可能にする。

---

# 5. スコープ

## 5.1 MVPに含める

* Jujutsuリポジトリの検出
* Jujutsuバージョンと能力の検出
* change graph表示
* operation graph表示
* operation選択による過去change graph表示
* change選択によるdiff表示
* operation間比較
* change ID、commit ID、bookmark、workspaceの表示
* conflict、empty、immutable、divergentの表示
* external operation changeの検知
* working-copy filesystem changeの検知
* snapshot鮮度表示
* read-only refresh
* explicit snapshot refresh
* undo
* capabilityが存在する場合のredo
* operation restore
* operation revert
* operation abandonのプレビュー
* 生のrevset入力
* operation logのlimit設定
* 実行した`jj`コマンドと結果の表示
* parse失敗時の診断表示

## 5.2 MVPで制限する操作

`op abandon`は回復可能性を低下させ得るため、初期段階では次のいずれかとする。

* プレビューのみ
* 実験機能として明示
* 詳細確認を経た限定的な実行

現在operationを含む範囲のabandonは、直接実行しない。

## 5.3 MVPに含めない

* GitHub Pull Request管理
* fetch、push、remote設定の完全なGUI化
* 内蔵コードエディタ
* 高度なmerge conflict editor
* `.jj`ストレージの直接変更
* 全revset構文のGUIビルダー
* AIによるdescription生成
* 自由なドラッグ＆ドロップ編集
* 全Jujutsuバージョンへの無条件対応

---

# 6. CLI実行・parse protocol

## 6.1 原則

Jujutsuのデフォルト表示はparseしない。

以下は機械可読入力として使用禁止とする。

* ユーザーのデフォルト`jj log`表示
* ユーザーのデフォルト`jj op log`表示
* 色付き出力
* pager経由の出力
* ユーザー定義templateに依存した出力
* graph decorationを含む人間向け出力

`jj log`と`jj op log`は`-T`で独自templateを指定でき、template言語にはJSONシリアライズ用の`json()`および文字列用の`escape_json()`がある。これを第一の機械可読化手段とする。

## 6.2 共通コマンドエンベロープ

すべての機械読み取りコマンドに、原則として次を付与する。

```text
--color=never
--no-pager
--quiet
--ignore-working-copy または明示的な --at-op
-T <vijutsu-owned-template>
```

環境には次を設定する。

```text
JJ_CONFIG=""
```

現在のJujutsuでは、`JJ_CONFIG`を空文字列にすると通常のuserおよびsystem configを読み込まない。さらに`--color=never`、`--no-pager`、`--config`などのglobal optionを明示する。

ただしrepo設定とworkspace設定が別レイヤとして存在するため、Vijutsuは次も行う。

* 出力形式を必ずコマンドラインで指定
* default revsetに依存しない
* default templateに依存しない
* diff toolに依存しない
* graph styleに依存しない
* 起動時にtemplate protocolのprobeを行う
* 期待したJSONLを返さない環境ではMutateを無効化する

Jujutsuの設定はbuilt-in、user、repo、workspace、command-lineの順で重ねられるため、出力に関係する設定は可能な限りcommand-lineで上書きする。

## 6.3 JSONL形式

基本形式は、1レコードにつき1 JSON objectとする。

```json
{"schema":1,"kind":"change","change_id":"...","commit_id":"..."}
{"schema":1,"kind":"change","change_id":"...","commit_id":"..."}
```

自由文を独自区切り文字で分割しない。

description、author、bookmark名、workspace名などはJSON文字列としてエスケープする。

### 必須フィールド

```text
schema
kind
source_jj_version
```

### parse規則

* 未知のフィールドは無視する
* 必須フィールド欠落はprotocol errorとする
* 未知のschema versionは明示的エラーとする
* 1レコード破損時に全体をpanicさせない
* stdoutとstderrを分離する
* trailing textを許容せず診断対象にする
* UTF-8不正をエラーとして扱う
* raw stdoutを診断画面から確認可能にする

## 6.4 ChangeGraph取得

change graphは、人間向けgraph線をparseせず、flat recordsからVijutsu側で構築する。

概念的な問い合わせは次の形とする。

```text
jj
  --at-op=<explicit-operation-id>
  --color=never
  --no-pager
  --quiet
  log
  --no-graph
  -r <explicit-revset>
  -T <change-jsonl-template>
```

各change recordには最低限次を含める。

```rust
struct ChangeRecord {
    change_id: String,
    commit_id: String,
    parent_commit_ids: Vec<String>,
    predecessor_commit_ids: Vec<String>,
    description: String,
    bookmarks: Vec<BookmarkRecord>,
    working_copies: Vec<String>,
    is_empty: bool,
    has_conflict: bool,
    is_immutable: bool,
    is_hidden: bool,
}
```

親commit IDから、Vijutsu側でDAGを構築する。

## 6.5 OperationGraph取得

operation graphも、人間向けgraph decorationをparseせず、flat recordsから構築する。

```text
jj
  --at-op=<explicit-operation-id-or-head>
  --color=never
  --no-pager
  --quiet
  op log
  --no-graph
  --limit <N>
  -T <operation-jsonl-template>
```

各operation recordには最低限次を含める。

```rust
struct OperationRecord {
    operation_id: String,
    parent_operation_ids: Vec<String>,
    description: String,
    timestamp_start: Option<String>,
    timestamp_end: Option<String>,
    user: Option<String>,
    host: Option<String>,
    is_current: bool,
}
```

利用中のJujutsuバージョンで必要なtemplate fieldが存在しない場合は、capabilityとして無効化する。

## 6.6 Diff取得

diff本文は、初期段階では表示対象であり、構造化分析の唯一の根拠にはしない。

* 選択changeのdiffは遅延取得する
* `--git`など明示した形式を使用する
* ユーザー設定のdiff toolを使用しない
* raw diffを表示する
* file listは別問い合わせから取得する
* raw diffのparse失敗でgraph表示を停止しない

## 6.7 設定汚染テスト

次を設定した環境でも起動できなければならない。

* 独自`templates.log`
* 独自`templates.op_log`
* 独自`revsets.log`
* 独自color
* 外部pager
* command alias
* template alias
* relative timestamp
* custom diff tool

---

# 7. ドメインモデル

## 7.1 RepositoryObservation

```rust
struct RepositoryObservation {
    observed_at: Instant,
    selected_operation_id: OperationId,
    operation_heads: Vec<OperationId>,
    operation_graph: OperationGraph,
    change_graph: ChangeGraph,
    freshness: ObservationFreshness,
}
```

## 7.2 ObservationFreshness

```rust
enum ObservationFreshness {
    CurrentSnapshot,
    FilesystemAhead {
        changed_paths_known: bool,
    },
    OperationHeadChanged,
    Historical {
        operation_id: OperationId,
    },
    StaleWorkingCopy,
    Unknown,
}
```

## 7.3 OperationNode

```rust
struct OperationNode {
    id: OperationId,
    parent_ids: Vec<OperationId>,
    description: String,
    timestamp: Option<TimestampRange>,
    user: Option<String>,
    host: Option<String>,
    is_head: bool,
    is_selected: bool,
}
```

## 7.4 ChangeNode

```rust
struct ChangeNode {
    change_id: ChangeId,
    commit_id: CommitId,
    parent_commit_ids: Vec<CommitId>,
    predecessor_commit_ids: Vec<CommitId>,
    description: String,
    bookmarks: Vec<Bookmark>,
    workspace_names: Vec<String>,
    flags: ChangeFlags,
}
```

## 7.5 OperationDelta

```rust
struct OperationDelta {
    from: OperationId,
    to: OperationId,
    added_commits: Vec<CommitId>,
    removed_commits: Vec<CommitId>,
    rewritten_changes: Vec<RewritePair>,
    parent_changes: Vec<ParentChange>,
    moved_bookmarks: Vec<BookmarkMove>,
    workspace_moves: Vec<WorkspaceMove>,
    conflict_transitions: Vec<ConflictTransition>,
}
```

---

# 8. OperationDelta計算戦略

## 8.1 第一方式：スナップショット比較

任意の二operationについて、それぞれ明示的な`--at-op=<id>`でChangeGraph snapshotを取得する。

```text
Snapshot A at operation OA
Snapshot B at operation OB
          ↓
Vijutsu internal graph diff
          ↓
OperationDelta
```

比較キーは次とする。

1. commit ID
2. change ID
3. predecessor relation
4. parent commit IDs
5. bookmark target
6. workspace target
7. conflict / empty状態

### 判定例

同じchange IDでcommit IDが変わった場合:

```text
rewritten change
```

同じchange IDでparentが変わった場合:

```text
rebaseまたは同等のgraph rewrite
```

change IDが一方にのみ存在する場合:

```text
added / removed / hidden
```

## 8.2 第二方式：`jj op diff` / `jj op show`

`jj op diff`と`jj op show`は、人間向けの詳細表示および第一方式の検証に使用する。

現在のCLIでは、`op diff`は任意の二operationを比較でき、`op show`はoperationとその親との差を表示する。ただし、その人間向け出力をVijutsuの主要IR生成手段にはしない。

## 8.3 Phase 0での評価項目

次を実験して、第一方式だけでは不足する情報を確認する。

* squash
* split
* rebase
* abandon
* bookmark move
* conflict発生
* conflict解消
* workspace move
* concurrent operation merge
* operation restore
* operation revert

必要な情報がtemplate APIから得られない場合のみ、`op diff`の限定parseを追加する。

---

# 9. Operation headと並行変更

## 9.1 外部変更の検知対象

Vijutsu起動中、次の変更を検知する。

* 別ターミナルでの`jj`実行
* 別workspaceでの`jj`実行
* AIエージェントによる`jj`実行
* background processによるsnapshot
* Git colocated repositoryの外部変更
* working-copy filesystem変更

## 9.2 OperationHeadWatcher

operation head変更を検知する抽象を設ける。

```rust
trait OperationHeadWatcher {
    fn poll_change(&mut self) -> WatchResult;
}
```

実装候補:

```text
FileBackendWatcher
PollingWatcher
HybridWatcher
```

Jujutsu公式FAQには、`.jj/repo/op_heads/heads`をwatchして変更時に再描画する例がある。

ただしop-heads storeはbackendとして抽象化されており、backend種別は`.jj/repo/op_heads/type`で選択される。したがって、特定ディレクトリ構造をVijutsu全体の前提にしない。

### FileBackendWatcher

対応する既知のfile backendでのみ使用する。

* op-head pathを監視
* 変更をrefresh triggerとして扱う
* ファイル内容をrepository stateとして直接解釈しない
* 監視失敗時はPollingWatcherへ移行する

### PollingWatcher

* 一定間隔で明示的なInspect queryを行う
* operation head ID集合の変化を比較する
* filesystem watcher非対応環境で使用する

## 9.3 Refresh trigger

refreshは次で発生する。

* ユーザーによるrefresh
* operation head変更
* working-copy filesystem変更
* Mutate command完了
* terminal focus復帰
* poll interval満了
* parse後にhead変化を検出した場合

## 9.4 TOCTOU対策

graph取得中にoperation headが変化する可能性がある。

取得手順:

```text
1. head集合 H1を取得
2. 明示operation IDでoperation graphを取得
3. 明示operation IDでchange graphを取得
4. head集合 H2を取得
5. H1 != H2なら観測結果をstaleとする
6. refreshを再試行
```

再試行回数には上限を設ける。高頻度変更時は、古い画面を維持しながら「repository changing」と表示する。

---

# 10. 複数operation head

## 10.1 基本方針

複数operation headがある状態で、Vijutsuが暗黙に統合を実行してはならない。

現在のCLIでは通常の最新状態読み込みはdivergent operationを統合し得る一方、`--at-op=@`は自動統合を行わない。

ただし、複数head時の詳細なCLI挙動はJujutsuバージョンに依存し得るため、起動時probeとPhase 0試験で確認する。

## 10.2 安全な取得手順

1. 非統合Inspect queryでoperation graphを取得
2. head集合を列挙
3. 各headを明示operation IDとして保持
4. UI上でheadを複数表示
5. ユーザーが一つのheadを選択
6. 選択headを`--at-op=<explicit-id>`で観測
7. 統合は明示的なMutate操作としてのみ実行

## 10.3 UI表示

```text
Operation heads: 2

◆ op-a  terminal-1: squash
◆ op-b  agent-2: describe

No operation was integrated.
Select a head to inspect.
```

複数headを一つの仮想状態として暗黙表示しない。

## 10.4 `@`の表示

Operationペインでは、単一の`@`記号を「唯一のoperation head」という意味で使用しない。

区別する。

```text
@   現在選択中の観測operation
◆   operation head
◎   working-copy workspaceが参照するoperation
```

---

# 11. Working copyと鮮度

## 11.1 Inspectモードの意味

`--ignore-working-copy`または過去operationの`--at-op`で表示される`@`は、最後にsnapshotされたworking-copy commitである。

ファイルシステムに未snapshot変更が存在する場合、表示diffは最新ではない。

## 11.2 必須表示

```text
Snapshot state: filesystem has newer changes
Displayed @: last Jujutsu snapshot
```

または、

```text
Snapshot state: current
Displayed @ matches the filesystem snapshot
```

過去operationの場合:

```text
Historical view
Operation: abc123
Working-copy filesystem is not reflected
```

## 11.3 Refresh種別

### Read-only refresh

* filesystemをsnapshotしない
* operationを生成しない
* 現在記録済みのviewのみ再取得する

### Snapshot refresh

* `jj util snapshot`など、検出したcapabilityに応じた明示的snapshotを実行
* 新しいoperationが生成され得る
* 実行前にその旨を表示する

## 11.4 File watcher

* 100〜500msでdebounce
* 一連のeditor saveを一イベントへまとめる
* refresh処理を直列化する
* 古い非同期結果を破棄する
* 変更pathは補助情報として扱う
* watcherのイベントだけでsnapshot済みと判断しない

---

# 12. 画面要件

## 12.1 基本レイアウト

```text
┌─ Repository ────────────────────────────────────────────┐
│ repo: vijutsu  ws: default  jj: x.y.z  heads: 1        │
│ view: current snapshot  fs: newer changes detected     │
└──────────────────────────────────────────────────────────┘

┌─ Operation graph ──────────┬─ Change graph ─────────────┐
│ @◆ ○ restore op-12        │ @ ○ qpvuntsm UI prototype │
│     ○ describe             │ │ ○ mzvwutvl Parse output │
│     ○ squash               │ ├─○ yostqsxw Experiment   │
│     ○ snapshot             │ ○ main Initial commit     │
└────────────────────────────┴─────────────────────────────┘

┌─ Files ────────────────────┬─ Diff / Operation delta ───┐
│ M src/app.rs               │ - old                      │
│ A src/graph.rs             │ + new                      │
└────────────────────────────┴─────────────────────────────┘
```

## 12.2 Operation graphペイン

表示項目:

* operation ID
* parent edge
* operation head
* 選択operation
* description
* timestamp
* user
* host
* 分岐
* 統合
* snapshot
* restore
* revert
* external process由来

## 12.3 Change graphペイン

表示項目:

* change ID
* commit ID
* parent edge
* working-copy marker
* bookmark
* workspace
* description
* empty
* conflict
* immutable
* divergent
* rewrite relation

## 12.4 Projection表示

operationを選択すると、そのoperation時点のchange graphを表示する。

```text
Viewing operation: op-12
Current operation head: op-19
Mode: read-only historical view
```

現在状態と誤認させない。

## 12.5 比較表示

二つのoperationを選択し、以下を表示する。

* added change
* removed change
* rewritten change
* commit ID変更
* parent変更
* bookmark移動
* workspace移動
* conflict発生・解消
* file diff

## 12.6 Revset入力

MVPに生のrevset入力欄を設ける。

```text
Revset: mutable()
```

要件:

* 入力値をshell経由で実行しない
* subprocess引数として直接渡す
* parse errorを画面内に表示する
* 最近使用したrevsetをsession内で保持する
* default revsetに戻す操作を設ける
* revset変更はInspectとして扱う

## 12.7 Operation limit

operation graphの表示件数を設定可能にする。

```text
Operation limit: 100
```

* 初期値を有限にする
* 追加読み込みを可能にする
* limit到達をUI上で表示する
* 古いoperationが存在しないように見せない

---

# 13. 操作別要件

## 13.1 Undo

* 対象operationを表示
* 逆適用されるchange graph差分を表示
* 新しいoperationが追加されることを表示
* 実行後に全状態を再取得する

## 13.2 Redo

capabilityが存在する場合のみ有効化する。

capabilityなしの場合:

```text
Redo is not supported by this Jujutsu version.
```

縮退動作:

* Undoは利用可能
* Redoキーは無効
* 代替として`op restore`候補を案内できる
* Vijutsu独自redoを実装しない

## 13.3 Operation Restore

```text
Operation graph:
新しいrestore operationが追加される

Repository view:
選択した過去operationの状態へ戻る

Later operations:
operation logには残る
```

現在のCLIでは`op restore`は、過去状態を復元する新しいoperationを作成する。

プレビュー:

```text
Current G19                 Restore target G12

@ D                         @ B
│                           │
○ C                         ○ A
│
○ B
│
○ A
```

必須警告:

```text
Later operations will not be deleted.
The working copy may be rewritten.
A new restore operation will be created.
```

## 13.4 Operation Revert

```text
選択operationのinverseを現在状態へ適用する。
```

現在のCLIでも`op revert`は、一operationの逆変換を新しいoperationとして適用する。

Restoreとの差を表示する。

```text
Restore:
選択時点のview全体を復元

Revert:
選択operation一件の効果を現在状態へ逆適用
```

## 13.5 Operation Abandon

表示意味:

```text
Operation history:
対象operationを到達可能な履歴から除く

Current repository view:
原則として維持される

Recoverability:
到達不能objectが将来GC対象になり得る
```

必須表示:

```text
Operation history changed: YES
Current view expected to change: NO
Objects may become unreachable: YES
Immediate physical deletion: NO
Deletion after garbage collection: POSSIBLE
```

GC後の喪失を断定しない。

## 13.6 Head integration

複数operation headの統合は、独立した明示操作とする。

* Inspectの副作用として実行しない
* 対象headを表示する
* 統合後のchange graphを可能な範囲でpreviewする
* capabilityがない場合はCLIでの手順のみ表示する

---

# 14. Capability検出

```rust
struct JjCapabilities {
    version: Version,
    json_template_function: bool,
    escape_json_method: bool,
    operation_parent_template: bool,
    commit_predecessor_template: bool,
    op_diff: bool,
    op_show: bool,
    op_revert: bool,
    op_restore: bool,
    op_restore_what: bool,
    redo: bool,
    util_snapshot: bool,
    no_integrate_operation: bool,
}
```

## 14.1 検出方法

起動時に以下を実行する。

1. `jj version`
2. template protocol probe
3. read-only operation query probe
4. change query probe
5. optional command help / capability probe

バージョン番号だけで能力を推定せず、可能な限り実際のコマンド成功で判定する。

## 14.2 縮退原則

* 観測不能な情報は`unknown`と表示
* 不明な状態を安全側に倒す
* preview不能なMutate操作を無効化
* read-only viewerとしては可能な限り起動
* バージョン不一致だけで全機能を停止しない

---

# 15. キー操作

```text
j / k       グラフ内で前後へ移動
h / l       グラフ内で親・子へ移動
Tab         次のペイン
Shift-Tab   前のペイン
Enter       詳細を開く
Space       比較対象を固定
/           Revset入力
[ / ]       Operation limitを変更
r           Read-only refresh
R           Snapshot refresh
u           Undo preview
Ctrl-r      Redo preview
s           Squash preview
b           Rebase preview
o           Operation viewへフォーカス
v           Operation revert preview
t           Operation restore preview
a           Operation abandon preview
:           Command palette
?           Help
q           Quit
```

`h/l`はグラフ移動専用とし、ペイン移動には使用しない。

---

# 16. 非機能要件

## 16.1 安定性

* 不正JSONLでpanicしない
* 選択ノード消失でpanicしない
* ファイル保存中にpanicしない
* operation head変更中にpanicしない
* 空リポジトリで起動できる
* conflict状態で起動できる
* 複数operation headで起動できる
* config汚染環境で起動できる
* parse不能時にraw outputを表示できる

## 16.2 性能目標

* 初回表示: 1秒以内を目標
* 通常refresh: 300ms以内を目標
* キー入力反応: 50ms以内
* diffは遅延取得
* graph snapshotはLRU cache
* operation limitを有限にする
* 外部変更連続時にrefreshを合流する

## 16.3 一貫性

一画面内に、異なるoperation由来の情報を混在させない。

全ペインは共通の`ObservationToken`を持つ。

```rust
struct ObservationToken {
    operation_id: OperationId,
    head_set_hash: Hash,
    observed_at: Instant,
}
```

tokenが一致しない非同期結果は破棄する。

## 16.4 可搬性

初期対象:

* Linux
* macOS

将来対象:

* Windows Terminal

filesystem watcherが利用不能な環境ではpollingへ縮退する。

## 16.5 アクセシビリティ

* 色だけで状態を区別しない
* 状態に文字と記号を併用
* ASCII-only mode
* narrow terminalではタブ式レイアウト
* 現在状態と過去状態を文言でも区別

---

# 17. 受け入れ条件

## AC-01 Change graph表示

複数changeを作成したリポジトリで、親子関係、change ID、commit ID、description、working copyを表示できる。

## AC-02 Diff連動

changeを選択すると、そのchangeのdiffを表示できる。

## AC-03 Operation projection

過去operationを選択すると、そのoperation時点のchange graphを表示できる。

## AC-04 JSONL parse

descriptionに改行、引用符、Unicode文字が含まれていても、正しくparseできる。

## AC-05 Config隔離

独自template、pager、color、revset、aliasを設定した環境でも同じ内部IRを生成できる。

## AC-06 Squash観測

squash前後で、消えたcommit、rewriteされたchange、移動したdiffを比較できる。

## AC-07 Restore

restore前に現在状態と対象状態を比較できる。実行後もrestore前のoperationを観測できる。

## AC-08 Abandon

対象範囲、operation graphへの効果、回復可能性への影響を表示できる。

## AC-09 ファイル変更耐性

起動中にファイルを連続保存してもクラッシュしない。未snapshot状態を正しく表示する。

## AC-10 外部operation変更

別ターミナルから`jj`を実行した場合、Vijutsuがoperation head変更を検知して再取得する。

## AC-11 二端末並行操作

二つのプロセスが異なるoperationを作成した場合、暗黙に統合せず、複数headを表示できる。

## AC-12 非変更観測

read-only refreshを繰り返しても、不必要なoperationを生成しない。

## AC-13 Head変更中の整合性

graph取得中にheadが変化した場合、異なる時点のペイン情報を混在させない。

## AC-14 Redo縮退

redo非対応バージョンでは、redoを無効化し、TUI全体は正常に動作する。

## AC-15 Revset

ユーザーが入力したrevsetでchange graphを絞り込み、構文エラーを画面内に表示できる。

## AC-16 Operation limit

operation件数を制限し、limit到達を明示し、追加読み込みできる。

## AC-17 エラー回復

`jj`コマンド失敗またはparse失敗後もTUIが終了せず、stderr、raw stdout、実行コマンドを確認できる。

---

# 18. Phase 0：意味論・protocol実験

## 18.1 基本操作

* edit
* new
* describe
* squash
* split
* rebase
* abandon
* undo
* redo
* op restore
* op revert
* op abandon
* conflict
* conflict resolution

## 18.2 Parse実験

* description内の改行
* 引用符
* バックスラッシュ
* Unicode
* 空description
* 長いdescription
* 複数bookmark
* conflicted bookmark
* hidden commit
* divergent change

## 18.3 Config汚染実験

* custom log template
* custom op-log template
* custom revset
* pager
* color
* command alias
* template alias
* custom graph style
* relative timestamp
* custom diff tool
* repo config
* workspace config

## 18.4 並行性実験

* Vijutsu起動中に別端末で`jj describe`
* 別端末で`jj squash`
* 別workspaceからrewrite
* 二つの長時間コマンド
* operation head分岐
* head統合
* graph取得中のhead変更
* 高頻度エージェント操作

## 18.5 OperationDelta実験

各操作について次を比較する。

```text
Internal snapshot diff
jj op diff
jj op show
Human expectation
```

目的は、snapshot比較だけで失われる意味情報を特定することである。

---

# 19. 開発段階

## Phase 0：Protocol laboratory

* txtリポジトリによる意味論実験
* JSONL template確定
* capability probe
* config隔離
* operation head検知
* snapshot diff検証

## Phase 1：Read-only change viewer

* change graph
* revset
* files
* diff
* stable selection
* snapshot鮮度表示

## Phase 2：Operation viewer

* operation graph
* operation limit
* explicit operation projection
* multiple heads
* operation comparison
* external change detection

## Phase 3：Safe mutation

* new
* edit
* describe
* squash
* undo
* optional redo
* restore
* revert

## Phase 4：History surgery

* rebase preview
* abandon preview
* head integration
* recoverability表示
* guided recent-history cleanup

## Phase 5：高度化

* workspace topology
* agentごとのchange表示
* worklog連携
* plugin API
* Refunctor連携
* optional `jj-lib` backend

---

# 20. 差別化

Vijutsuは、単なるJujutsu用コマンドランチャーではない。

中心構造は次である。

> operation graphの各ノードを選択すると、そのoperationが保持するchange graph全体が表示される。

さらに、各操作を三種類の効果へ分解する。

```text
1. Change graphへの効果
2. Operation graphへの効果
3. 回復可能性への効果
```

そして、観測時には第四の軸を追加する。

```text
4. 表示状態の鮮度と観測時点
```

Vijutsuは、Jujutsuという状態遷移系に対する、二階のグラフ観測器兼エディタである。

---

# 21. v0.2凍結条件

以下がPhase 0で確認できた時点で、要件v0.2を凍結する。

* JSONL protocolが主要対象バージョンで動作する
* config汚染環境で同一IRを生成できる
* explicit operation IDによる副作用なし観測が成立する
* 複数operation headを暗黙統合せず列挙できる
* external operation変更を検知できる
* snapshot比較で主要なOperationDeltaを再構成できる
* 不足情報と`op diff`依存範囲が確定する
* read-only refreshがoperationを生成しない
* filesystemとsnapshotの乖離を検知・表示できる
