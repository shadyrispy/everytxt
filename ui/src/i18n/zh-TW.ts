/**
 * 繁體中文資源（批次 D / P4，台灣用語慣例）。key 與 zh-CN.ts 嚴格同步。
 */
const zhTW: Record<string, string> = {
  /* MenuBar */
  'menu.menubar': '選單列',
  'menu.file': '檔案(F)',
  'menu.file.export': '匯出結果',
  'menu.file.exit': '結束',
  'menu.edit': '編輯(E)',
  'menu.edit.copyPath': '複製路徑',
  'menu.edit.copyName': '複製檔案名稱',
  'menu.edit.selectAll': '全選結果',
  'menu.edit.findInDoc': '在文件中搜尋',
  'menu.view': '檢視(V)',
  'menu.view.themeLight': '淺色主題',
  'menu.view.themeDark': '深色主題',
  'menu.view.themeSystem': '跟隨系統',
  'menu.view.prevPage': '上一個結果',
  'menu.view.nextPage': '下一個結果',
  'menu.view.lineNumbers': '顯示行號',
  'menu.view.wrap': '自動換行',
  'menu.view.singlePreview': '單一文件預覽',
  'menu.view.zoomIn': '放大',
  'menu.view.zoomOut': '縮小',
  'menu.view.zoomReset': '重設字號',
  'menu.settings': '選項(O)',
  'menu.settings.options': '設定…',
  'menu.settings.indexAdmin': '索引管理…',
  'menu.help': '說明(H)',
  'menu.help.syntax': '進階搜尋語法速查',
  'menu.help.homepage': '專案首頁',
  'menu.help.about': '關於 search-tool',

  /* MenuBar：進階搜尋語法速查面板 */
  'syntax.title': '進階搜尋語法',
  'syntax.header.op': '語法',
  'syntax.header.meaning': '意義',
  'syntax.header.example': '範例說明',
  'syntax.and.name': '且',
  'syntax.and.desc': '同時包含左右兩側關鍵字，如：error & timeout',
  'syntax.or.name': '或',
  'syntax.or.desc': '包含任一側關鍵字，如：cat | dog',
  'syntax.not.name': '非',
  'syntax.not.desc': '排除右側關鍵字，如：report !draft',
  'syntax.group.name': '群組',
  'syntax.group.desc': '將子運算式分組，如：(a|b) & c',
  'syntax.exact.name': '精確',
  'syntax.exact.desc': '精確片語比對，如："中華民國"',
  'syntax.hint': '將搜尋模式切換到「進階」後即可使用語法；多個詞預設以「且」處理。',

  /* StatusBar（文案在 App.tsx 組合，key 留給整合方） */
  'status.searching': '正在搜尋…',
  'status.ready': '就緒',
  'status.results': '傳回 {1} 個結果，耗時 {2} 秒，搜尋完成。',
  'status.indexDocs': '{1} 篇 · {2}',
  'status.indexUnavailable': '索引無法使用，請重建',
  'status.indexLoading': '索引載入中…',

  /* IndexAdminDialog：B4 查找/瀏覽新增 */
  'idx.addFind': '搜尋新增…',
  'idx.addBrowse': '瀏覽新增…',
  'idx.findTypePlaceholder': '輸入類型名稱，如 chm',
  'idx.stagedTypes': '已暫存',
  'idx.stagedEmpty': '點擊上方清單以暫存類型',
  'idx.confirmAddTypes': '確定',
  'idx.cancel': '取消',
  'idx.unstage': '取消暫存',
  'idx.noMatch': '沒有符合的類型',
  'idx.noExt': '無法從檔名判別副檔名',
  'idx.unsupported': '不支援的格式：{1}（尚未支援內容擷取）',
  'idx.addedTypes': '已新增 {1} 個類型',

  /* FilterRow：D2 類型描述 */
  'idx.allTypes': '不限檔案類型',

  /* SelectionBar：D1 全部刪除 */
  'sel.deleteAll': '全部刪除',
  'sel.deleteAllConfirm': '將刪除全部 {1} 個搜尋結果檔案（移至資源回收筒，可還原）。確定繼續？',

  /* WelcomePage */
  'welcome.title': '歡迎使用',
  'welcome.steps.title': '搜尋步驟',
  'welcome.step1': '在左邊的搜尋框輸入想搜尋的任意關鍵字；',
  'welcome.step2': '點擊左邊的「開始」按鈕，或按 Enter，搜尋結果會立刻呈現；',
  'welcome.step3': '點擊搜尋結果中的項目，右側將產生該檔案的預覽；',
  'welcome.step4': '點擊上方的「+」按鈕，還可以依資料夾與檔案類型篩選結果。',
  'welcome.tips.title': '使用技巧',
  'welcome.tip1': '在任何資料夾上按右鍵選「用 search-tool 搜尋」，可快速依該資料夾搜尋；',
  'welcome.tip2': '開啟系統匣與全域熱鍵，隨時喚起主視窗進行搜尋；',
  'welcome.tip3': '滑鼠停留在結果上 1 秒，可快速預覽命中的文字片段；',
  'welcome.tip4': '在「設定 → 快速鍵」中可自訂喚醒等操作的快速鍵。',
  'welcome.more': '……更多實用、高效的功能等你探索。',
  'welcome.about.title': '關於 everytxt',
  'welcome.about.body1': 'everytxt 是一款本機全文搜尋引擎：安全、無廣告、資料不外洩，請放心使用。',
  'welcome.about.body2':
    'everytxt 免費且開放原始碼：索引與檢索全在本機完成，文件內容不會離開你的電腦。歡迎透過「說明」選單提出意見。',
  'welcome.openHomepage': '前往專案首頁',
  /* ---- 7.9 / B6：檢查更新 ---- */
  'update.check': '檢查更新',
  'update.checking': '正在檢查更新…',
  'update.latest': '目前已是最新版本（{version}）',
  'update.available': '發現新版本 {version}（目前 {current}）',
  'update.availableNotes': '更新說明：\n{notes}',
  'update.installConfirm': '發現新版本 {version}，是否現在更新？\n應用程式將下載並自動重新啟動。',
  'update.installing': '正在下載並安裝更新，應用程式即將重新啟動…',
  'update.failed': '更新檢查失敗：{error}',
  'update.notConfigured': '更新通道未設定（首個正式版發布後啟用）',
}

export default zhTW
