use clap::Parser;

/// AI CLI（Gemini、Codex、Claude）を使用したスマートコミットメッセージ生成ツール
#[derive(Parser, Debug)]
#[command(name = "git-sc")]
#[command(about = "AI CLI（Gemini、Codex、Claude）を使用したスマートコミットメッセージ生成ツール")]
#[command(version)]
pub struct Cli {
    /// 確認プロンプトをスキップして直接コミット
    #[arg(short = 'y', long = "yes")]
    pub auto_confirm: bool,

    /// 実際にコミットせずに生成されたメッセージを表示
    #[arg(short = 'n', long = "dry-run")]
    pub dry_run: bool,

    /// アンステージの変更も含めて全てをステージングしてコミット
    #[arg(short = 'a', long = "all")]
    pub stage_all: bool,

    /// 直前のコミットを新しく生成されたメッセージで修正
    #[arg(long = "amend")]
    pub amend: bool,

    /// ブランチ内の全コミットを1つにまとめて新しいメッセージを生成（ベースブランチを指定）
    #[arg(long = "squash", value_name = "BASE")]
    pub squash: Option<String>,

    /// コミットメッセージの言語（設定ファイルを上書き）
    #[arg(short = 'l', long = "lang")]
    pub language: Option<String>,
}
