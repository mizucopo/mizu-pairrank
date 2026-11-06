// Exact native error messages only. Never apply this dictionary to user content.
export const backendEn: Record<string, string> = {
  "保存先がありません。": "No storage location is available.",
  "保存データのファイル名がありません。": "The saved data has no file name.",
  "インポートする画像と項目の数が一致しません。":
    "The number of imported images does not match the number of items.",
  "インポートするタグが見つかりません。": "An imported tag was not found.",
  "タグの複製に失敗しました。": "Could not duplicate the tags.",
  "項目名を入力してください。": "Enter an item name.",
  "異なる2項目を選択してください。": "Select two different items.",
  "比較する項目が見つかりません。": "The items to compare were not found.",
  "評価値の計算に失敗しました。回答は保存していません。":
    "Could not calculate ratings. Your answer was not saved.",
  "回答の保存形式が不正です。": "The saved answer format is invalid.",
  "取り消せる回答がありません。": "There is no answer to undo.",
  "取り消す回答が見つかりません。": "The answer to undo was not found.",
  "保存先が変更されました。アプリを再起動してください。":
    "The storage location has changed. Restart the app.",
  "名前を入力してください。": "Enter a name.",
  "項目が見つかりません。": "The item was not found.",
  "タグが見つかりません。": "The tag was not found.",
  "同じ名前のタグが既にあります。": "A tag with this name already exists.",
  "リストが見つかりません。": "The list was not found.",
  "保存済みの評価モデルに対応していません。対応するアプリを使用してください。":
    "The saved rating model is not supported. Use a compatible version of the app.",
  "バージョン未管理のデータベースです。既存データを保護するため更新を中止しました。":
    "The database has no version information. The update was stopped to protect existing data.",
  "アプリに含まれるマイグレーションの連番が不正です。":
    "The app's database migration sequence is invalid.",
  "データベースの更新で参照関係の不整合が検出されたため、変更を取り消しました。":
    "The database update detected inconsistent references and rolled back the changes.",
  "対応していないリスト形式・バージョンです。": "This list format or version is not supported.",
  "リストは5,000項目・1,000タグまで取り込めます。":
    "Lists can contain up to 5,000 items and 1,000 tags when imported.",
  "同じ名前のタグが重複しています。": "Duplicate tag names were found.",
  "タグの割り当ては合計50,000件まで取り込めます。": "Up to 50,000 tag assignments can be imported.",
  "項目のタグが未定義、または重複しています。": "An item's tags are undefined or duplicated.",
  "画像は1枚20MiBまで取り込めます。": "Each imported image must be 20 MiB or smaller.",
  "画像のBase64データが不正です。": "The image's Base64 data is invalid.",
  "画像は20MiB以下のPNGデータにしてください。": "Images must be PNG data no larger than 20 MiB.",
  "画像は500枚まで含められます。画像なしでも共有できます。":
    "Up to 500 images can be included. You can also share without images.",
  "PNGの画像サイズを読み取れませんでした。": "Could not read the PNG dimensions.",
  "画像のサイズ・画素数が上限を超えています。":
    "The image dimensions or pixel count exceed the limit.",
  "画像は合計1億画素まで含められます。画像なしでも共有できます。":
    "Images can total up to 100 million pixels. You can also share without images.",
  "画像は500枚まで含められます。画像を含めずにエクスポートしてください。":
    "Up to 500 images can be included. Export without images.",
  "画像の保存先を開けませんでした。": "Could not open the image storage location.",
  "50MiBを超えます。画像を含めずにエクスポートしてください。":
    "The size exceeds 50 MiB. Export without images.",
  "リストを書き出せませんでした。": "Could not export the list.",
  "選択したリストファイルを確認できませんでした。": "Could not verify the selected list file.",
  "通常のリストファイルを選択してください。": "Select a regular list file.",
  "選択したリストファイルを開けませんでした。": "Could not open the selected list file.",
  "50MiB以下のリストファイルを選択してください。": "Select a list file no larger than 50 MiB.",
  "選択したリストファイルを読み取れませんでした。": "Could not read the selected list file.",
  "対応するZIP形式ではないか、ファイルが破損しています。":
    "The ZIP format is unsupported or the file is damaged.",
  "展開後も50MiB以下のリストファイルを選択してください。":
    "Select a list file no larger than 50 MiB after extraction.",
  "ZIPの展開サイズが不正、または50MiBを超えています。":
    "The ZIP's extracted size is invalid or exceeds 50 MiB.",
  "リスト形式が不正、またはファイルが破損しています。":
    "The list format is invalid or the file is damaged.",
  "保存先の拡張子は .zip にしてください。": "Use the .zip extension for the destination file.",
  "通常のファイルを保存先に選択してください。": "Select a regular file as the destination.",
  "保存先を確認できませんでした。": "Could not verify the destination.",
  "リストを保存できませんでした。保存先の権限と空き容量を確認してください。":
    "Could not save the list. Check destination permissions and free space.",
  "画像を書き出せませんでした。画像なしで再試行できます。":
    "Could not export images. You can try again without images.",
  "保存先が起動中に変更されました。アプリを再起動してください。":
    "The storage location changed during startup. Restart the app.",
  "データ処理を続けられません。アプリを再起動してください。":
    "Data processing cannot continue. Restart the app.",
  "データ操作を続けられません。アプリを再起動してください。":
    "Data operations cannot continue. Restart the app.",
  "保存先の選択を完了できませんでした。": "Could not finish selecting a destination.",
  "レポートを保存できませんでした。再試行してください。": "Could not save the report. Try again.",
  "ファイルの選択を完了できませんでした。": "Could not finish selecting a file.",
  "リストが更新されています。最新の比較を読み直してください。":
    "The list has changed. Reload the latest comparison.",
  "比較項目が見つかりません。": "The comparison items were not found.",
  "ブラウザーを開けませんでした。": "Could not open the browser.",
  "レポートはバイナリデータで送信してください。再試行してください。":
    "Reports must be sent as binary data. Try again.",
  "レポート形式の指定が正しくありません。再試行してください。":
    "The requested report format is invalid. Try again.",
  "レポートの表示種類が正しくありません。再試行してください。":
    "The requested report view is invalid. Try again.",
  "画像は500枚まで含められます。画像なしでも保存できます。":
    "Up to 500 images can be included. You can also save without images.",
  "リストが更新されています。ランキングを読み直して再試行してください。":
    "The list has changed. Reload the ranking and try again.",
  "画像の項目が重複しています。ランキングを読み直して再試行してください。":
    "Image items are duplicated. Reload the ranking and try again.",
  "項目の画像が見つかりません。ランキングを読み直すか、画像なしで再試行してください。":
    "An item's image was not found. Reload the ranking or try again without images.",
  "項目が見つかりません。ランキングを読み直して再試行してください。":
    "An item was not found. Reload the ranking and try again.",
  "画像が50MiBを超えます。画像なしで再試行してください。":
    "Images exceed 50 MiB. Try again without images.",
  "レポートは50MiBまで保存できます。画像なしで再試行してください。":
    "Reports can be up to 50 MiB. Try again without images.",
  "レポートの形式が正しくありません。ランキングを読み直して再試行してください。":
    "The report format is invalid. Reload the ranking and try again.",
  "レポートを保存できませんでした。保存先の権限と空き容量を確認してください。":
    "Could not save the report. Check destination permissions and free space.",
  "画像URLの名前解決が実行中です。少し待ってから再試行してください。":
    "Image URL name resolution is in progress. Wait briefly and try again.",
  "画像URLの名前解決がタイムアウトしました。": "Image URL name resolution timed out.",
  "画像URLの名前解決に失敗しました。": "Could not resolve the image URL's host name.",
  "画像の保存フォルダーを開けませんでした。": "Could not open the image folder.",
  "画像の保存先ロックを開けませんでした。": "Could not open the image storage lock.",
  "未使用画像を確認できませんでした。": "Could not check for unused images.",
  "画像の保存先ロックを解除できませんでした。": "Could not release the image storage lock.",
  "画像の保存先をロックできませんでした。": "Could not lock image storage.",
  "画像の保存先が変更されています。アプリを再起動してください。":
    "The image storage location has changed. Restart the app.",
  "検索設定の読み込みが実行中です。少し待ってから再試行してください。":
    "Search settings are being loaded. Wait briefly and try again.",
  "検索設定を取得できませんでした。": "Could not load search settings.",
  "画像の登録が実行中です。少し待ってから再試行してください。":
    "An image is being added. Wait briefly and try again.",
  "APIキーの形式が正しくありません。": "The API key format is invalid.",
  "APIキーの保存が実行中です。少し待ってから再試行してください。":
    "An API key is being saved. Wait briefly and try again.",
  "検索設定またはモデル一覧の取得が実行中です。少し待ってから再試行してください。":
    "Search settings or models are being loaded. Wait briefly and try again.",
  "検索設定の保存が実行中です。少し待ってから再試行してください。":
    "Search settings are being saved. Wait briefly and try again.",
  "OpenAIの検索設定を保存できませんでした。": "Could not save OpenAI search settings.",
  "検索語は1〜400文字、50語以内で入力してください。":
    "Enter a search query of 1–400 characters and no more than 50 words.",
  "画像候補の取得がタイムアウトしました。検索語を変えて再試行してください。":
    "Loading image candidates timed out. Change the search query and try again.",
  "画像検索がタイムアウトしました。時間をおいて再試行してください。":
    "Image search timed out. Try again later.",
  "画像検索が実行中です。少し待ってから再試行してください。":
    "Image search is in progress. Wait briefly and try again.",
  "OpenAIの認証情報の取得が実行中です。": "OpenAI credentials are being loaded.",
  "APIキーを取得できませんでした。": "Could not retrieve the API key.",
  "選択した検索サービスのAPIキーを設定してください。":
    "Set an API key for the selected search service.",
  "画像処理を続けられません。アプリを再起動してください。":
    "Image processing cannot continue. Restart the app.",
  "画像保存処理を完了できませんでした。": "Could not finish saving the image.",
  "選択した画像ファイルを開けませんでした。": "Could not open the selected image file.",
  "選択した画像ファイルを確認できませんでした。": "Could not verify the selected image file.",
  "20MiB以下の画像ファイルを選択してください。": "Select an image file no larger than 20 MiB.",
  "選択した画像ファイルを読み取れませんでした。": "Could not read the selected image file.",
  "登録画像を読み取れませんでした。画像を含めずに再試行できます。":
    "Could not read a saved image. You can try again without images.",
  "検索サービスの応答を読み取れませんでした。": "Could not read the search service response.",
  "検索サービスの応答形式が正しくありません。": "The search service response format is invalid.",
  "ページに代表画像がありません。": "The page has no representative image.",
  "画像のプレビューを作成できませんでした。": "Could not create an image preview.",
  "画像を読み取れませんでした。PNG・JPEG・WebPに対応しています。":
    "Could not read the image. PNG, JPEG, and WebP are supported.",
  "PNG・JPEG・WebPの画像を選択してください。": "Select a PNG, JPEG, or WebP image.",
  "画像サイズを読み取れませんでした。": "Could not read the image dimensions.",
  "画像の画素数が上限（3,200万画素）を超えています。":
    "The image exceeds the limit of 32 million pixels.",
  "画像は合計1億画素まで含められます。画像を含めずにエクスポートしてください。":
    "Images can total up to 100 million pixels. Export without images.",
  "画像を読み取れませんでした。画像が破損していないか確認してください。":
    "Could not read the image. Check whether it is damaged.",
  "画像を保存形式に変換できませんでした。": "Could not convert the image to the storage format.",
  "画像の保存先を作成できませんでした。": "Could not create the image storage location.",
  "画像ファイルを保存できませんでした。空き容量を確認してください。":
    "Could not save the image file. Check free space.",
  "画像の取得がタイムアウトしました。": "Loading the image timed out.",
  "検索接続を準備できませんでした。": "Could not prepare the search connection.",
  "検索サービスに接続できませんでした。通信状態を確認してください。":
    "Could not connect to the search service. Check your connection.",
  "画像URLにホスト名がありません。": "The image URL has no host name.",
  "画像接続を準備できませんでした。": "Could not prepare the image connection.",
  "画像または検索元ページを取得できませんでした。": "Could not retrieve the image or source page.",
  "画像URLのリダイレクト回数が上限を超えました。": "The image URL exceeded the redirect limit.",
  "リダイレクト先のURLを読み取れませんでした。": "Could not read the redirect URL.",
  "画像または検索元ページのサーバーが取得を拒否しました。":
    "The image or source page server denied the request.",
  "OpenAIの検索要求はResponses APIで作成してください。":
    "Use the Responses API for OpenAI search requests.",
  "検索要求を作成できませんでした。APIキーを確認してください。":
    "Could not create the search request. Check the API key.",
  "検索サービスの認証に失敗しました。APIキーを確認してください。":
    "Search service authentication failed. Check the API key.",
  "検索サービスの利用上限に達しました。時間をおいて再試行してください。":
    "The search service usage limit was reached. Try again later.",
  "検索サービスでエラーが発生しました。時間をおいて再試行してください。":
    "The search service returned an error. Try again later.",
  "取得データがサイズ上限を超えています。": "The downloaded data exceeds the size limit.",
  "取得データを最後まで読み取れませんでした。": "Could not finish reading the downloaded data.",
  "画像URLが正しくありません。": "The image URL is invalid.",
  "画像は公開されたHTTP/HTTPSのURLから取得してください。":
    "Images must use publicly accessible HTTP or HTTPS URLs.",
  "リダイレクト先のURLが正しくありません。": "The redirect URL is invalid.",
  "ローカル・プライベートネットワークのURLは取得できません。":
    "Local or private network URLs cannot be accessed.",
  "保存されたChatGPTアカウント登録が不正です。":
    "The saved ChatGPT account registration is invalid.",
  "ChatGPT接続のHTTPクライアントを作成できませんでした。":
    "Could not create the ChatGPT HTTP client.",
  "ChatGPTサインインのローカル受信口を開けませんでした。":
    "Could not open the local ChatGPT sign-in listener.",
  "ChatGPTサインインのローカル受信口を確認できませんでした。":
    "Could not verify the local ChatGPT sign-in listener.",
  "ChatGPTの認証先が不正です。": "The ChatGPT authentication destination is invalid.",
  "ChatGPTサインインが時間切れになりました。再試行してください。":
    "ChatGPT sign-in timed out. Try again.",
  "ChatGPTの本人確認トークンがありません。再試行してください。":
    "The ChatGPT identity token is missing. Try again.",
  "選択したChatGPTアカウントとサインインしたアカウントが一致しません。新しいアカウントとして接続してください。":
    "The selected ChatGPT account does not match the signed-in account. Connect as a new account.",
  "選択したChatGPTアカウント登録がありません。":
    "The selected ChatGPT account registration was not found.",
  "ChatGPTのサインアウトを完了できませんでした。": "Could not finish signing out of ChatGPT.",
  "このアプリからサインアウトしましたが、ChatGPT側のセッション失効を確認できませんでした。ChatGPTの設定からアプリの接続を解除してください。":
    "You have signed out of this app, but revocation of the ChatGPT session could not be confirmed. Disconnect the app in ChatGPT settings.",
  "ChatGPT接続の更新を完了できませんでした。":
    "Could not finish refreshing the ChatGPT connection.",
  "先にChatGPTに接続してください。": "Connect to ChatGPT first.",
  "ChatGPTプランの使用が有効になっていません。プランの使用を許可して接続し直してください。":
    "ChatGPT plan usage is not enabled. Allow plan usage and reconnect.",
  "ChatGPTに接続し直してください。": "Reconnect to ChatGPT.",
  "ChatGPTの接続を更新できません。サインインし直してください。":
    "Could not refresh the ChatGPT connection. Sign in again.",
  "ChatGPTプランの使用許可がなくなりました。接続し直してください。":
    "ChatGPT plan usage permission was revoked. Reconnect.",
  "ChatGPTの接続処理が進行中です。完了してから再試行してください。":
    "A ChatGPT connection operation is in progress. Wait until it finishes and try again.",
  "ChatGPT認証の発行元が一致しません。": "The ChatGPT authentication issuer does not match.",
  "ChatGPT認証の接続先が不正です。": "The ChatGPT authentication endpoint is invalid.",
  "ChatGPT認証の接続先が許可された発行元ではありません。":
    "The ChatGPT authentication endpoint is not an allowed issuer.",
  "ChatGPT認証サービスに接続できませんでした。再試行してください。":
    "Could not connect to the ChatGPT authentication service. Try again.",
  "ChatGPTの接続を更新できませんでした。通信状態を確認して再試行してください。":
    "Could not refresh the ChatGPT connection. Check your connection and try again.",
  "ChatGPTの接続が失効しました。サインインし直してください。":
    "The ChatGPT connection has expired. Sign in again.",
  "ChatGPTのクライアント登録が無効です。新しいアカウントとして接続してください。":
    "The ChatGPT client registration is invalid. Connect as a new account.",
  "失効処理の接続先がありません。": "The revocation endpoint is missing.",
  "ChatGPT側の失効を確認できませんでした。": "Could not confirm revocation on ChatGPT.",
  "ChatGPT接続のローカル処理を完了できませんでした。":
    "Could not finish the local ChatGPT connection operation.",
  "ChatGPT接続を保存できませんでした。": "Could not save the ChatGPT connection.",
  "ChatGPTアカウント登録の保存容量を超えました。既存の登録は保持されています。":
    "ChatGPT account registration storage is full. Existing registrations have been preserved.",
  "保存されたChatGPTホストIDが不正です。": "The saved ChatGPT host ID is invalid.",
  "保存されたChatGPT接続の形式が不正です。": "The saved ChatGPT connection format is invalid.",
  "保存されたChatGPT接続を読み取れませんでした。": "Could not read the saved ChatGPT connection.",
  "ChatGPTの接続トークンの応答が不完全です。接続し直してください。":
    "The ChatGPT connection token response is incomplete. Reconnect.",
  "端末の日時を確認してください。": "Check your device's date and time.",
  "ChatGPTトークンの有効期限が不正です。": "The ChatGPT token expiration time is invalid.",
  "安全なサインイン情報を生成できませんでした。": "Could not generate secure sign-in information.",
  "ChatGPT認証サービスの応答が大きすぎます。":
    "The ChatGPT authentication service response is too large.",
  "ChatGPT認証サービスの応答を受信できませんでした。":
    "Could not receive the ChatGPT authentication service response.",
  "ChatGPT認証サービスの応答形式が不正です。":
    "The ChatGPT authentication service response format is invalid.",
  "ChatGPTの本人確認を検証できませんでした。接続し直してください。":
    "Could not verify your ChatGPT identity. Reconnect.",
  "ChatGPTサインインがキャンセルされました。必要な場合は再試行してください。":
    "ChatGPT sign-in was canceled. Try again if needed.",
  "ChatGPTの認証コードがありません。": "The ChatGPT authorization code is missing.",
  "ChatGPTのクライアント登録が一致しません。": "The ChatGPT client registration does not match.",
  "ChatGPTのクライアント登録が完了しませんでした。再試行してください。":
    "ChatGPT client registration did not finish. Try again.",
  "ChatGPTサインインの応答を受信できませんでした。":
    "Could not receive the ChatGPT sign-in response.",
  "ChatGPT接続を他のアプリプロセスと同期できませんでした。":
    "Could not synchronize the ChatGPT connection with other app processes.",
  "OSの資格情報ストアを開けませんでした。": "Could not open the operating system credential store.",
  "ChatGPT接続のプロセス間ロックを開けませんでした。":
    "Could not open the ChatGPT connection inter-process lock.",
  "ChatGPT接続の保存先が差し替えられました。":
    "The ChatGPT connection storage location was replaced.",
  "ChatGPT資格情報のプロセス間ロックを開けませんでした。":
    "Could not open the ChatGPT credentials inter-process lock.",
  "OSの資格情報ストアから資格情報を取得できませんでした。":
    "Could not retrieve credentials from the operating system credential store.",
  "資格情報を削除できませんでした。": "Could not delete credentials.",
  "資格情報をOSの資格情報ストアに保存できませんでした。":
    "Could not save credentials in the operating system credential store.",
  "OpenAIのモデルIDは空白・制御文字を含まない128バイト以内の文字列で指定してください。":
    "The OpenAI model ID must be no more than 128 bytes with no whitespace or control characters.",
  "保存されたOpenAI設定の形式が不正です。設定を確認して保存し直してください。":
    "The saved OpenAI settings format is invalid. Check your settings and save again.",
  "保存されたOpenAI設定を読み取れませんでした。設定を確認して保存し直してください。":
    "Could not read saved OpenAI settings. Check your settings and save again.",
  "OpenAI設定を保存できませんでした。": "Could not save OpenAI settings.",
  "OpenAIへの接続を準備できませんでした。": "Could not prepare the OpenAI connection.",
  "OpenAIへの通信がタイムアウトしました。再試行してください。":
    "The OpenAI request timed out. Try again.",
  "OpenAIに接続できませんでした。通信状態を確認して再試行してください。":
    "Could not connect to OpenAI. Check your connection and try again.",
  "OpenAI設定を取得できませんでした。": "Could not load OpenAI settings.",
  "OpenAIのAPIキーを設定してください。": "Set an OpenAI API key.",
  "OpenAIのAPIキーを取得できませんでした。": "Could not retrieve the OpenAI API key.",
  "画像の検索語は400文字・50語以内で入力してください。":
    "Image search queries must be no more than 400 characters and 50 words.",
  "OpenAIのモデル一覧の要求を作成できませんでした。":
    "Could not create the OpenAI model list request.",
  "OpenAIのモデル一覧を読み取れませんでした。": "Could not read the OpenAI model list.",
  "OpenAIの画像検索の要求を作成できませんでした。":
    "Could not create the OpenAI image search request.",
  "OpenAIの応答がサイズ上限を超えました。": "The OpenAI response exceeded the size limit.",
  "OpenAIのモデル一覧の形式が不正です。": "The OpenAI model list format is invalid.",
  "ChatGPTプランの利用上限に達しました。「利用状況を管理」で確認してください。":
    "Your ChatGPT plan usage limit was reached. Check Manage usage.",
  "ChatGPTプランの使用を現在利用できません。「利用状況を管理」で確認して再試行してください。":
    "ChatGPT plan usage is currently unavailable. Check Manage usage and try again.",
  "ChatGPTプランの使用が選択したアカウント・ワークスペースまたはポリシーで制限されています。利用条件をアカウント・ワークスペースの管理者に確認してください。":
    "ChatGPT plan usage is restricted by the selected account, workspace, or policy. Check usage conditions with your account or workspace administrator.",
  "ChatGPTプランの接続経路が利用を許可されていません。アプリの更新・対応状況を確認してください。":
    "This ChatGPT plan connection route is not allowed. Check app updates and support.",
  "ChatGPTプランの使用権限を確認できませんでした。選択したアカウントとアプリの接続・許可設定を確認してください。":
    "Could not verify ChatGPT plan usage permission. Check the selected account and the app's connection and permission settings.",
  "OpenAIのAPIキーを認証できませんでした。APIキーを確認して保存し直してください。":
    "Could not authenticate the OpenAI API key. Check it and save again.",
  "ChatGPTの接続を認証できませんでした。接続し直してください。":
    "Could not authenticate the ChatGPT connection. Reconnect.",
  "OpenAIのリクエスト上限に達しました。時間をおいて再試行してください。":
    "The OpenAI request limit was reached. Try again later.",
  "ChatGPTプランの利用が権限・ポリシーまたは提供地域の制限で許可されませんでした。選択したアカウント・ワークスペースとアプリの利用許可を確認してください。":
    "ChatGPT plan usage was denied due to permissions, policy, or regional restrictions. Check the selected account, workspace, and app permissions.",
  "OpenAIの画像検索を完了できませんでした。再試行してください。":
    "Could not finish the OpenAI image search. Try again.",
  "OpenAIのストリームの形式が不正です。": "The OpenAI stream format is invalid.",
  "OpenAIのストリームを読み取れませんでした。": "Could not read the OpenAI stream.",
  "OpenAIのストリームのイベント種別が一致しません。":
    "The OpenAI stream event type does not match.",
  "OpenAIのストリームの完了イベントが重複しています。":
    "The OpenAI stream contains duplicate completion events.",
  "OpenAIの検索結果の形式が不正です。": "The OpenAI search result format is invalid.",
  "OpenAIの画像検索が未完了で終了しました。再試行してください。":
    "The OpenAI image search ended incomplete. Try again.",
  "OpenAIのストリームが途中で切断されました。再試行してください。":
    "The OpenAI stream was interrupted. Try again.",
  "OpenAIの画像検索が完了する前にストリームが終了しました。再試行してください。":
    "The stream ended before the OpenAI image search finished. Try again.",
  "OpenAIの画像検索ツールが完了していません。再試行してください。":
    "The OpenAI image search tool has not finished. Try again.",
  "評価値は有限の μ と正の有限の σ が必要です。":
    "Ratings require a finite μ and a positive finite σ.",
  "評価値が数値計算で扱える範囲を超えています。":
    "Ratings exceed the range supported by numerical calculations.",
  "評価値の更新を安全に計算できませんでした。": "Could not safely calculate the rating update.",
  "比較候補の情報量を安全に計算できませんでした。":
    "Could not safely calculate comparison candidate information gain.",
  "比較候補に重複 ID または不正な評価値があります。":
    "Comparison candidates contain duplicate IDs or invalid ratings.",
  "比較結果の区間を数値計算で表現できませんでした。":
    "Could not represent the comparison result interval numerically.",
  "比較結果の分布を安全に計算できませんでした。":
    "Could not safely calculate the comparison result distribution.",
  "保存データのファイル名が不正です。": "The saved data file name is invalid.",
  "保存ファイル名が単一のエントリではありません。":
    "The storage file name is not a single path entry.",
  "保存ファイル名が長すぎます。": "The storage file name is too long.",
  "保存先に reparse point は使用できません。":
    "Reparse points cannot be used as storage destinations.",
  "ディレクトリエントリが不正です。": "The directory entry is invalid.",
  "画像ファイルの場所がディレクトリです。": "The image file location is a directory.",
  "保存データの権限を安全に復元できません。所有者の読み取り権限（推奨 600）を手動で復元して再試行してください。":
    "Could not safely restore saved data permissions. Manually restore owner read permission (recommended: 600) and try again.",
  "副ファイルの所有者の読み取り権限を手動で復元して再試行してください。":
    "Manually restore owner read permission on the auxiliary file and try again.",
  "Windows SQLite VFS に syscall 検証機能がありません。":
    "The Windows SQLite VFS lacks syscall verification support.",
  "Windows SQLite VFS に syscall 登録機能がありません。":
    "The Windows SQLite VFS lacks syscall registration support.",
  "Windows SQLite syscall はすでに初期化されています。":
    "Windows SQLite syscalls are already initialized.",
  "Windows資格情報ストアのChatGPT接続記録が不完全または不正です。":
    "The ChatGPT connection record in the Windows credential store is incomplete or invalid.",
  "Windows資格情報ストアのChatGPT接続を読み取れませんでした。":
    "Could not read the ChatGPT connection from the Windows credential store.",
  "Windows資格情報ストアのChatGPT接続を更新できませんでした。":
    "Could not update the ChatGPT connection in the Windows credential store.",
  "ChatGPT接続記録が大きすぎるため保存できませんでした。":
    "The ChatGPT connection record is too large to save.",
  "ChatGPT接続の参照は削除しましたが、一部の資格情報がOSに残っています。Windows資格情報マネージャーからmizu-pairrankのChatGPT資格情報を削除してください。":
    "The ChatGPT connection reference was deleted, but some credentials remain in the OS. Delete mizu-pairrank's ChatGPT credentials in Windows Credential Manager.",
  "ChatGPT接続を保存できませんでした。以前の接続は保持されています。再試行してください。":
    "Could not save the ChatGPT connection. The previous connection has been preserved. Try again.",
  "ChatGPT接続を保存できず、以前の接続は保持されています。一部の未使用資格情報がOSに残りました。Windows資格情報マネージャーで確認してください。":
    "Could not save the ChatGPT connection. The previous connection has been preserved, but some unused credentials remain in the OS. Check Windows Credential Manager.",
  "ChatGPT接続は更新されましたが、一部の古い資格情報がOSに残っています。Windows資格情報マネージャーでmizu-pairrankの古いChatGPT資格情報を削除してください。":
    "The ChatGPT connection was updated, but some old credentials remain in the OS. Delete mizu-pairrank's old ChatGPT credentials in Windows Credential Manager.",
  "OSの資格情報ストアからChatGPT資格情報を取得できませんでした。":
    "Could not retrieve ChatGPT credentials from the operating system credential store.",
  "ChatGPT資格情報をOSの資格情報ストアに保存できませんでした。":
    "Could not save ChatGPT credentials in the operating system credential store.",
  "OSの資格情報ストアからChatGPT資格情報を削除できませんでした。":
    "Could not delete ChatGPT credentials from the operating system credential store.",
};
