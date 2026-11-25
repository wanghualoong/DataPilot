/** Enter that only commits an input-method composition (Sogou / Pinyin "回车上屏英文"): must not send or submit.
 *  WebKit (macOS WKWebView) reports that keydown with isComposing = false but keyCode 229, so check both. */
export function imeEnter(e: { key?: string; keyCode?: number; nativeEvent?: { isComposing?: boolean; keyCode?: number }; isComposing?: boolean }): boolean {
  const n = e.nativeEvent ?? e;
  return !!(n.isComposing || e.isComposing || e.keyCode === 229 || n.keyCode === 229);
}
