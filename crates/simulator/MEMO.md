シミュレータがやるべきことは

```systemverilog
module Top (
    a: input logic<32>,
    b: input logic<32>,
    c: output logic<32>,
){
    assign c = a + b;
}
```

のようなVerylのソースコードに対して

```rust
let mut sim = Simulator::new("Top");
sim.set("a", 10);
sim.set("b", 20);
sim.step();
assert_eq!(sim.get("c"), 30);
```

のようなRustのコードが通るようにすることです。
サンプルとして、`./src/tests.rs` にこのテストを書いておきました。


シミュレータが持つべき情報は

* 変数テーブル（a,b,cの現在の値を保持する）
* 実行する文のテーブル（上の例ではassign文）

になると思います。
シミュレータの実行が行うことは

* 変数テーブルの全変数に未評価のフラグをつける（newの時点）
* setで変数に値をセットし未評価フラグをクリア
* 変数テーブルから未評価の変数を取ってくる（ここではcだけが未評価）
* 取ってきた変数に対応する文（ここではassign）を評価する
* 文の評価中に未評価の変数が現れたら、それを深さ優先で再帰的に評価する
* 未評価の変数がなくなったら終了

のようになります。RTLではソースコードの上から下に実行されるのではなく、各文は並列に実行される（つまりコードの下の方に書かれた文の結果が上の方にも影響する）モデルなので、このような実行方法になります。

ソースコードから変数テーブルと文のテーブルを構築するにはシンボルテーブルを参照します。
シンボルテーブルは名前に対応するシンボル情報を持っています。
Symbol構造体の定義は以下になります。

https://docs.rs/veryl-analyzer/latest/veryl_analyzer/symbol/struct.Symbol.html

シンボルの種類はSymbolKindにあり、

https://docs.rs/veryl-analyzer/latest/veryl_analyzer/symbol/enum.SymbolKind.html

各バリアント内の構造体に、その種類固有の情報が入っています。
例えばModulePropertyにはdefinitionがあり（definitionを追加したバージョンはまだリリースしていないのでdocs.rsにはないです）、ここからモジュール定義の構文木全体を取得できるので、そこからassign文を抜き出せます。
ここは将来的にはコンパイラの解析フェーズでシミュレータが必要な情報を事前に抜いてくるのもいいかもしれません。
変数はSymbolKindがPort/Variableのものです。それがTopモジュール内にあるかどうかはnamespaceで確認できます。

Verylコンパイラ内では文字列などヒープを要するリソースはコピーや借用などが面倒なので全てID（usize）で管理していて、
実体はスレッドローカルストレージ内のHashMapにあります。なので実体を参照したいときはテーブルから引いてくる必要があります。
具体的なコードは `./src/tests.rs` にいくつか書いておきました。

また、式の評価はEvaluatorの実装が参考になると思います。将来的にはコンパイラ用とシミュレータ用は統合した方が良いかもしれませんが、まだ要件がはっきりしないので、とりあえずはシミュレータ専用に作るのが楽だと思います。

https://docs.rs/veryl-analyzer/latest/veryl_analyzer/evaluator/struct.Evaluator.html

進め方についてはとりあえず最初の例のような簡単なものから始めて

* always_comb文
* クロックの導入（always_ff文）
* モジュール階層
* 関数呼出し

という感じで徐々に機能を増やしていくのがいいと思います。

あとRTLで扱う変数はとても大きくなることがよくある（256bitとか1024bitとか）ので、最初はusizeでもいいですが早いうちに何らかのbig integerクレートに移行するのがいいと思います。

構文定義は以下になります。この定義からRustのコードが生成されるので、書かれているノード名とRustの構造体名は同じになります。
取ってきたい構文要素がどのように入っているかを追うにはこちらが便利かもしれません。

https://github.com/veryl-lang/veryl/blob/master/crates/parser/veryl.par
