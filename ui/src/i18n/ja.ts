/**
 * 日本語リソース（批次 D / P4）。key は zh-CN.ts と厳密に一致させること。
 */
const ja: Record<string, string> = {
  /* MenuBar */
  'menu.menubar': 'メニューバー',
  'menu.file': 'ファイル(F)',
  'menu.file.export': '結果をエクスポート',
  'menu.file.exit': '終了',
  'menu.edit': '編集(E)',
  'menu.edit.copyPath': 'パスをコピー',
  'menu.edit.copyName': 'ファイル名をコピー',
  'menu.edit.selectAll': 'すべて選択',
  'menu.edit.findInDoc': 'ドキュメント内を検索',
  'menu.view': '表示(V)',
  'menu.view.themeLight': 'ライトテーマ',
  'menu.view.themeDark': 'ダークテーマ',
  'menu.view.themeSystem': 'システムに従う',
  'menu.view.prevPage': '前の結果',
  'menu.view.nextPage': '次の結果',
  'menu.view.lineNumbers': '行番号を表示',
  'menu.view.wrap': '折り返し',
  'menu.view.singlePreview': '単一ドキュメントプレビュー',
  'menu.view.zoomIn': '拡大',
  'menu.view.zoomOut': '縮小',
  'menu.view.zoomReset': 'フォントサイズをリセット',
  'menu.settings': 'オプション(O)',
  'menu.settings.options': '設定…',
  'menu.settings.indexAdmin': 'インデックス管理…',
  'menu.help': 'ヘルプ(H)',
  'menu.help.syntax': '高度な検索構文',
  'menu.help.homepage': 'プロジェクトホーム',
  'menu.help.about': 'search-tool について',

  /* MenuBar：高度な検索構文パネル */
  'syntax.title': '高度な検索構文',
  'syntax.header.op': '構文',
  'syntax.header.meaning': '意味',
  'syntax.header.example': '例',
  'syntax.and.name': 'AND',
  'syntax.and.desc': '左右の両方のキーワードを含む。例：error & timeout',
  'syntax.or.name': 'OR',
  'syntax.or.desc': 'いずれか一方のキーワードを含む。例：cat | dog',
  'syntax.not.name': 'NOT',
  'syntax.not.desc': '右側のキーワードを除外する。例：report !draft',
  'syntax.group.name': 'グループ',
  'syntax.group.desc': '部分式をグループ化する。例：(a|b) & c',
  'syntax.exact.name': '完全一致',
  'syntax.exact.desc': '完全な語句で一致させる。例："東京都"',
  'syntax.hint':
    '検索モードを「詳細」に切り替えると構文が使えます。複数の語は既定で AND で結合されます。',

  /* StatusBar（App.tsx で組み立て、key は統合側が保持） */
  'status.searching': '検索中…',
  'status.ready': '準備完了',
  'status.results': '{1} 件の結果、{2} 秒で検索が完了しました。',
  'status.indexDocs': '{1} 件 · {2}',
  'status.indexUnavailable': 'インデックスが利用できません。再構築してください',
  'status.indexLoading': 'インデックス読み込み中…',

  /* IndexAdminDialog：B4 検索/参照で追加 */
  'idx.addFind': '検索して追加…',
  'idx.addBrowse': '参照して追加…',
  'idx.findTypePlaceholder': '種類名を入力（例：chm）',
  'idx.stagedTypes': 'ステージ済み',
  'idx.stagedEmpty': '上のリストをクリックしてステージします',
  'idx.confirmAddTypes': 'OK',
  'idx.cancel': 'キャンセル',
  'idx.unstage': 'ステージ解除',
  'idx.noMatch': '一致する種類がありません',
  'idx.noExt': 'ファイル名から拡張子を特定できません',
  'idx.unsupported': '未対応の形式：{1}（内容の抽出には未対応）',
  'idx.addedTypes': '{1} 個の種類を追加しました',
  /* E-1（O5）：索引構成 */
  'idx.compLoadFail': '索引構成の読み込みに失敗しました：{1}',
  'idx.compNoExt': '（拡張子なし）',

  /* FilterRow：D2 種類の説明 */
  'idx.allTypes': 'すべてのファイル種類',

  /* SelectionBar：D1 すべて削除 */
  'sel.deleteAll': 'すべて削除',
  'sel.deleteAllConfirm':
    '検索結果のファイル {1} 件をすべて削除します（ゴミ箱へ移動、復元可能）。続行しますか？',

  /* WelcomePage */
  'welcome.title': 'ようこそ',
  'welcome.steps.title': '検索の手順',
  'welcome.step1': '左の検索ボックスに探したいキーワードを入力します；',
  'welcome.step2':
    '左の「開始」ボタンをクリックするか Enter キーを押すと、結果がすぐに表示されます；',
  'welcome.step3': '結果の項目をクリックすると、右側にファイルのプレビューが表示されます；',
  'welcome.step4':
    '上部の「+」ボタンで、ディレクトリやファイル種類による絞り込みもできます。',
  'welcome.tips.title': '使い方のコツ',
  'welcome.tip1':
    'フォルダを右クリックして「search-tool で検索」を選ぶと、そのフォルダ内を素早く検索できます；',
  'welcome.tip2':
    'トレイアイコンとグローバルホットキーを有効にすると、いつでもメインウィンドウを呼び出せます；',
  'welcome.tip3': '結果の上に 1 秒ホバーすると、一致したテキスト断片を素早くプレビューできます；',
  'welcome.tip4': '「設定 → ショートカット」で起動などのキーをカスタマイズできます。',
  'welcome.more': '…その他の便利な機能もぜひお試しください。',
  'welcome.about.title': 'everytxt について',
  'welcome.about.body1':
    'everytxt はローカル全文検索エンジンです：安全、広告なし、データ漏えいなし。安心してご利用ください。',
  'welcome.about.body2':
    'everytxt は無料かつオープンソースです：インデックス作成も検索もすべてローカルで行われ、ドキュメントの内容があなたの PC の外に出ることはありません。ご意見は「ヘルプ」メニューからどうぞ。',
  'welcome.openHomepage': 'プロジェクトホームを開く',
  /* ---- 7.9 / B6：更新の確認 ---- */
  'update.check': '更新を確認',
  'update.checking': '更新を確認中…',
  'update.latest': '最新バージョンです（{version}）',
  'update.available': '新バージョン {version} があります（現在 {current}）',
  'update.availableNotes': '更新内容：\n{notes}',
  'update.installConfirm':
    '新バージョン {version} があります。今すぐ更新しますか？\nアプリがダウンロードして自動的に再起動します。',
  'update.installing': '更新をダウンロードしてインストールしています。まもなく再起動します…',
  'update.failed': '更新の確認に失敗しました：{error}',
  'update.notConfigured': '更新チャネルが未設定です（正式版リリース後に有効化）',
}

export default ja
