/*!
  Highlight.js v11.7.0 (git: bc1b06bb3a)
  (c) 2006-2024 undefined and other contributors
  License: BSD-3-Clause
 */
var hljs=function(){"use strict";var e={exports:{}};function n(e){
return e instanceof Map?e.clear=e.delete=e.set=()=>{
throw Error("map is read-only")}:e instanceof Set&&(e.add=e.clear=e.delete=()=>{
throw Error("set is read-only")
}),Object.freeze(e),Object.getOwnPropertyNames(e).forEach((t=>{var i=e[t]
;"object"!=typeof i||Object.isFrozen(i)||n(i)})),e}
e.exports=n,e.exports.default=n;class t{constructor(e){
void 0===e.data&&(e.data={}),this.data=e.data,this.isMatchIgnored=!1}
ignoreMatch(){this.isMatchIgnored=!0}}function i(e){
return e.replace(/&/g,"&amp;").replace(/</g,"&lt;").replace(/>/g,"&gt;").replace(/"/g,"&quot;").replace(/'/g,"&#x27;")
}function r(e,...n){const t=Object.create(null);for(const n in e)t[n]=e[n]
;return n.forEach((e=>{for(const n in e)t[n]=e[n]})),t}
const s=e=>!!e.scope||e.sublanguage&&e.language;class a{constructor(e,n){
this.buffer="",this.classPrefix=n.classPrefix,e.walk(this)}addText(e){
this.buffer+=i(e)}openNode(e){if(!s(e))return;let n=""
;n=e.sublanguage?"language-"+e.language:((e,{prefix:n})=>{if(e.includes(".")){
const t=e.split(".")
;return[`${n}${t.shift()}`,...t.map(((e,n)=>`${e}${"_".repeat(n+1)}`))].join(" ")
}return`${n}${e}`})(e.scope,{prefix:this.classPrefix}),this.span(n)}
closeNode(e){s(e)&&(this.buffer+="</span>")}value(){return this.buffer}span(e){
this.buffer+=`<span class="${e}">`}}const o=(e={})=>{const n={children:[]}
;return Object.assign(n,e),n};class l{constructor(){
this.rootNode=o(),this.stack=[this.rootNode]}get top(){
return this.stack[this.stack.length-1]}get root(){return this.rootNode}add(e){
this.top.children.push(e)}openNode(e){const n=o({scope:e})
;this.add(n),this.stack.push(n)}closeNode(){
if(this.stack.length>1)return this.stack.pop()}closeAllNodes(){
for(;this.closeNode(););}toJSON(){return JSON.stringify(this.rootNode,null,4)}
walk(e){return this.constructor._walk(e,this.rootNode)}static _walk(e,n){
return"string"==typeof n?e.addText(n):n.children&&(e.openNode(n),
n.children.forEach((n=>this._walk(e,n))),e.closeNode(n)),e}static _collapse(e){
"string"!=typeof e&&e.children&&(e.children.every((e=>"string"==typeof e))?e.children=[e.children.join("")]:e.children.forEach((e=>{
l._collapse(e)})))}}class c extends l{constructor(e){super(),this.options=e}
addKeyword(e,n){""!==e&&(this.openNode(n),this.addText(e),this.closeNode())}
addText(e){""!==e&&this.add(e)}addSublanguage(e,n){const t=e.root
;t.sublanguage=!0,t.language=n,this.add(t)}toHTML(){
return new a(this,this.options).value()}finalize(){return!0}}function d(e){
return e?"string"==typeof e?e:e.source:null}function g(e){return f("(?=",e,")")}
function u(e){return f("(?:",e,")*")}function h(e){return f("(?:",e,")?")}
function f(...e){return e.map((e=>d(e))).join("")}function p(...e){const n=(e=>{
const n=e[e.length-1]
;return"object"==typeof n&&n.constructor===Object?(e.splice(e.length-1,1),n):{}
})(e);return"("+(n.capture?"":"?:")+e.map((e=>d(e))).join("|")+")"}
function b(e){return RegExp(e.toString()+"|").exec("").length-1}
const m=/\[(?:[^\\\]]|\\.)*\]|\(\??|\\([1-9][0-9]*)|\\./
;function $(e,{joinWith:n}){let t=0;return e.map((e=>{t+=1;const n=t
;let i=d(e),r="";for(;i.length>0;){const e=m.exec(i);if(!e){r+=i;break}
r+=i.substring(0,e.index),
i=i.substring(e.index+e[0].length),"\\"===e[0][0]&&e[1]?r+="\\"+(Number(e[1])+n):(r+=e[0],
"("===e[0]&&t++)}return r})).map((e=>`(${e})`)).join(n)}
const _="[a-zA-Z]\\w*",y="[a-zA-Z_]\\w*",w="\\b\\d+(\\.\\d+)?",E="(-?)(\\b0[xX][a-fA-F0-9]+|(\\b\\d+(\\.\\d*)?|\\.\\d+)([eE][-+]?\\d+)?)",x="\\b(0b[01]+)",v={
begin:"\\\\[\\s\\S]",relevance:0},k={scope:"string",begin:"'",end:"'",
illegal:"\\n",contains:[v]},O={scope:"string",begin:'"',end:'"',illegal:"\\n",
contains:[v]},N=(e,n,t={})=>{const i=r({scope:"comment",begin:e,end:n,
contains:[]},t);i.contains.push({scope:"doctag",
begin:"[ ]*(?=(TODO|FIXME|NOTE|BUG|OPTIMIZE|HACK|XXX):)",
end:/(TODO|FIXME|NOTE|BUG|OPTIMIZE|HACK|XXX):/,excludeBegin:!0,relevance:0})
;const s=p("I","a","is","so","us","to","at","if","in","it","on",/[A-Za-z]+['](d|ve|re|ll|t|s|n)/,/[A-Za-z]+[-][a-z]+/,/[A-Za-z][a-z]{2,}/)
;return i.contains.push({begin:f(/[ ]+/,"(",s,/[.]?[:]?([.][ ]|[ ])/,"){3}")}),i
},M=N("//","$"),S=N("/\\*","\\*/"),A=N("#","$");var j=Object.freeze({
__proto__:null,MATCH_NOTHING_RE:/\b\B/,IDENT_RE:_,UNDERSCORE_IDENT_RE:y,
NUMBER_RE:w,C_NUMBER_RE:E,BINARY_NUMBER_RE:x,
RE_STARTERS_RE:"!|!=|!==|%|%=|&|&&|&=|\\*|\\*=|\\+|\\+=|,|-|-=|/=|/|:|;|<<|<<=|<=|<|===|==|=|>>>=|>>=|>=|>>>|>>|>|\\?|\\[|\\{|\\(|\\^|\\^=|\\||\\|=|\\|\\||~",
SHEBANG:(e={})=>{const n=/^#![ ]*\//
;return e.binary&&(e.begin=f(n,/.*\b/,e.binary,/\b.*/)),r({scope:"meta",begin:n,
end:/$/,relevance:0,"on:begin":(e,n)=>{0!==e.index&&n.ignoreMatch()}},e)},
BACKSLASH_ESCAPE:v,APOS_STRING_MODE:k,QUOTE_STRING_MODE:O,PHRASAL_WORDS_MODE:{
begin:/\b(a|an|the|are|I'm|isn't|don't|doesn't|won't|but|just|should|pretty|simply|enough|gonna|going|wtf|so|such|will|you|your|they|like|more)\b/
},COMMENT:N,C_LINE_COMMENT_MODE:M,C_BLOCK_COMMENT_MODE:S,HASH_COMMENT_MODE:A,
NUMBER_MODE:{scope:"number",begin:w,relevance:0},C_NUMBER_MODE:{scope:"number",
begin:E,relevance:0},BINARY_NUMBER_MODE:{scope:"number",begin:x,relevance:0},
REGEXP_MODE:{begin:/(?=\/[^/\n]*\/)/,contains:[{scope:"regexp",begin:/\//,
end:/\/[gimuy]*/,illegal:/\n/,contains:[v,{begin:/\[/,end:/\]/,relevance:0,
contains:[v]}]}]},TITLE_MODE:{scope:"title",begin:_,relevance:0},
UNDERSCORE_TITLE_MODE:{scope:"title",begin:y,relevance:0},METHOD_GUARD:{
begin:"\\.\\s*[a-zA-Z_]\\w*",relevance:0},END_SAME_AS_BEGIN:e=>Object.assign(e,{
"on:begin":(e,n)=>{n.data._beginMatch=e[1]},"on:end":(e,n)=>{
n.data._beginMatch!==e[1]&&n.ignoreMatch()}})});function R(e,n){
"."===e.input[e.index-1]&&n.ignoreMatch()}function I(e,n){
void 0!==e.className&&(e.scope=e.className,delete e.className)}function T(e,n){
n&&e.beginKeywords&&(e.begin="\\b("+e.beginKeywords.split(" ").join("|")+")(?!\\.)(?=\\b|\\s)",
e.__beforeBegin=R,e.keywords=e.keywords||e.beginKeywords,delete e.beginKeywords,
void 0===e.relevance&&(e.relevance=0))}function L(e,n){
Array.isArray(e.illegal)&&(e.illegal=p(...e.illegal))}function B(e,n){
if(e.match){
if(e.begin||e.end)throw Error("begin & end are not supported with match")
;e.begin=e.match,delete e.match}}function C(e,n){
void 0===e.relevance&&(e.relevance=1)}const D=(e,n)=>{if(!e.beforeMatch)return
;if(e.starts)throw Error("beforeMatch cannot be used with starts")
;const t=Object.assign({},e);Object.keys(e).forEach((n=>{delete e[n]
})),e.keywords=t.keywords,e.begin=f(t.beforeMatch,g(t.begin)),e.starts={
relevance:0,contains:[Object.assign(t,{endsParent:!0})]
},e.relevance=0,delete t.beforeMatch
},H=["of","and","for","in","not","or","if","then","parent","list","value"]
;function P(e,n,t="keyword"){const i=Object.create(null)
;return"string"==typeof e?r(t,e.split(" ")):Array.isArray(e)?r(t,e):Object.keys(e).forEach((t=>{
Object.assign(i,P(e[t],n,t))})),i;function r(e,t){
n&&(t=t.map((e=>e.toLowerCase()))),t.forEach((n=>{const t=n.split("|")
;i[t[0]]=[e,z(t[0],t[1])]}))}}function z(e,n){
return n?Number(n):(e=>H.includes(e.toLowerCase()))(e)?0:1}const U={},K=e=>{
console.error(e)},q=(e,...n)=>{console.log("WARN: "+e,...n)},X=(e,n)=>{
U[`${e}/${n}`]||(console.log(`Deprecated as of ${e}. ${n}`),U[`${e}/${n}`]=!0)
},Z=Error();function F(e,n,{key:t}){let i=0;const r=e[t],s={},a={}
;for(let e=1;e<=n.length;e++)a[e+i]=r[e],s[e+i]=!0,i+=b(n[e-1])
;e[t]=a,e[t]._emit=s,e[t]._multi=!0}function G(e){(e=>{
e.scope&&"object"==typeof e.scope&&null!==e.scope&&(e.beginScope=e.scope,
delete e.scope)})(e),"string"==typeof e.beginScope&&(e.beginScope={
_wrap:e.beginScope}),"string"==typeof e.endScope&&(e.endScope={_wrap:e.endScope
}),(e=>{if(Array.isArray(e.begin)){
if(e.skip||e.excludeBegin||e.returnBegin)throw K("skip, excludeBegin, returnBegin not compatible with beginScope: {}"),
Z
;if("object"!=typeof e.beginScope||null===e.beginScope)throw K("beginScope must be object"),
Z;F(e,e.begin,{key:"beginScope"}),e.begin=$(e.begin,{joinWith:""})}})(e),(e=>{
if(Array.isArray(e.end)){
if(e.skip||e.excludeEnd||e.returnEnd)throw K("skip, excludeEnd, returnEnd not compatible with endScope: {}"),
Z
;if("object"!=typeof e.endScope||null===e.endScope)throw K("endScope must be object"),
Z;F(e,e.end,{key:"endScope"}),e.end=$(e.end,{joinWith:""})}})(e)}function W(e){
function n(n,t){
return RegExp(d(n),"m"+(e.case_insensitive?"i":"")+(e.unicodeRegex?"u":"")+(t?"g":""))
}class t{constructor(){
this.matchIndexes={},this.regexes=[],this.matchAt=1,this.position=0}
addRule(e,n){
n.position=this.position++,this.matchIndexes[this.matchAt]=n,this.regexes.push([n,e]),
this.matchAt+=b(e)+1}compile(){0===this.regexes.length&&(this.exec=()=>null)
;const e=this.regexes.map((e=>e[1]));this.matcherRe=n($(e,{joinWith:"|"
}),!0),this.lastIndex=0}exec(e){this.matcherRe.lastIndex=this.lastIndex
;const n=this.matcherRe.exec(e);if(!n)return null
;const t=n.findIndex(((e,n)=>n>0&&void 0!==e)),i=this.matchIndexes[t]
;return n.splice(0,t),Object.assign(n,i)}}class i{constructor(){
this.rules=[],this.multiRegexes=[],
this.count=0,this.lastIndex=0,this.regexIndex=0}getMatcher(e){
if(this.multiRegexes[e])return this.multiRegexes[e];const n=new t
;return this.rules.slice(e).forEach((([e,t])=>n.addRule(e,t))),
n.compile(),this.multiRegexes[e]=n,n}resumingScanAtSamePosition(){
return 0!==this.regexIndex}considerAll(){this.regexIndex=0}addRule(e,n){
this.rules.push([e,n]),"begin"===n.type&&this.count++}exec(e){
const n=this.getMatcher(this.regexIndex);n.lastIndex=this.lastIndex
;let t=n.exec(e)
;if(this.resumingScanAtSamePosition())if(t&&t.index===this.lastIndex);else{
const n=this.getMatcher(0);n.lastIndex=this.lastIndex+1,t=n.exec(e)}
return t&&(this.regexIndex+=t.position+1,
this.regexIndex===this.count&&this.considerAll()),t}}
if(e.compilerExtensions||(e.compilerExtensions=[]),
e.contains&&e.contains.includes("self"))throw Error("ERR: contains `self` is not supported at the top-level of a language.  See documentation.")
;return e.classNameAliases=r(e.classNameAliases||{}),function t(s,a){const o=s
;if(s.isCompiled)return o
;[I,B,G,D].forEach((e=>e(s,a))),e.compilerExtensions.forEach((e=>e(s,a))),
s.__beforeBegin=null,[T,L,C].forEach((e=>e(s,a))),s.isCompiled=!0;let l=null
;return"object"==typeof s.keywords&&s.keywords.$pattern&&(s.keywords=Object.assign({},s.keywords),
l=s.keywords.$pattern,
delete s.keywords.$pattern),l=l||/\w+/,s.keywords&&(s.keywords=P(s.keywords,e.case_insensitive)),
o.keywordPatternRe=n(l,!0),
a&&(s.begin||(s.begin=/\B|\b/),o.beginRe=n(o.begin),s.end||s.endsWithParent||(s.end=/\B|\b/),
s.end&&(o.endRe=n(o.end)),
o.terminatorEnd=d(o.end)||"",s.endsWithParent&&a.terminatorEnd&&(o.terminatorEnd+=(s.end?"|":"")+a.terminatorEnd)),
s.illegal&&(o.illegalRe=n(s.illegal)),
s.contains||(s.contains=[]),s.contains=[].concat(...s.contains.map((e=>(e=>(e.variants&&!e.cachedVariants&&(e.cachedVariants=e.variants.map((n=>r(e,{
variants:null},n)))),e.cachedVariants?e.cachedVariants:V(e)?r(e,{
starts:e.starts?r(e.starts):null
}):Object.isFrozen(e)?r(e):e))("self"===e?s:e)))),s.contains.forEach((e=>{t(e,o)
})),s.starts&&t(s.starts,a),o.matcher=(e=>{const n=new i
;return e.contains.forEach((e=>n.addRule(e.begin,{rule:e,type:"begin"
}))),e.terminatorEnd&&n.addRule(e.terminatorEnd,{type:"end"
}),e.illegal&&n.addRule(e.illegal,{type:"illegal"}),n})(o),o}(e)}function V(e){
return!!e&&(e.endsWithParent||V(e.starts))}class Q extends Error{
constructor(e,n){super(e),this.name="HTMLInjectionError",this.html=n}}
const J=i,Y=r,ee=Symbol("nomatch");var ne=(n=>{
const i=Object.create(null),r=Object.create(null),s=[];let a=!0
;const o="Could not find the language '{}', did you forget to load/include a language module?",l={
disableAutodetect:!0,name:"Plain text",contains:[]};let d={
ignoreUnescapedHTML:!1,throwUnescapedHTML:!1,noHighlightRe:/^(no-?highlight)$/i,
languageDetectRe:/\blang(?:uage)?-([\w-]+)\b/i,classPrefix:"hljs-",
cssSelector:"pre code",languages:null,__emitter:c};function b(e){
return d.noHighlightRe.test(e)}function m(e,n,t){let i="",r=""
;"object"==typeof n?(i=e,
t=n.ignoreIllegals,r=n.language):(X("10.7.0","highlight(lang, code, ...args) has been deprecated."),
X("10.7.0","Please use highlight(code, options) instead.\nhttps://github.com/highlightjs/highlight.js/issues/2277"),
r=e,i=n),void 0===t&&(t=!0);const s={code:i,language:r};O("before:highlight",s)
;const a=s.result?s.result:$(s.language,s.code,t)
;return a.code=s.code,O("after:highlight",a),a}function $(e,n,r,s){
const l=Object.create(null);function c(){if(!k.keywords)return void N.addText(M)
;let e=0;k.keywordPatternRe.lastIndex=0;let n=k.keywordPatternRe.exec(M),t=""
;for(;n;){t+=M.substring(e,n.index)
;const r=w.case_insensitive?n[0].toLowerCase():n[0],s=(i=r,k.keywords[i]);if(s){
const[e,i]=s
;if(N.addText(t),t="",l[r]=(l[r]||0)+1,l[r]<=7&&(S+=i),e.startsWith("_"))t+=n[0];else{
const t=w.classNameAliases[e]||e;N.addKeyword(n[0],t)}}else t+=n[0]
;e=k.keywordPatternRe.lastIndex,n=k.keywordPatternRe.exec(M)}var i
;t+=M.substring(e),N.addText(t)}function g(){null!=k.subLanguage?(()=>{
if(""===M)return;let e=null;if("string"==typeof k.subLanguage){
if(!i[k.subLanguage])return void N.addText(M)
;e=$(k.subLanguage,M,!0,O[k.subLanguage]),O[k.subLanguage]=e._top
}else e=_(M,k.subLanguage.length?k.subLanguage:null)
;k.relevance>0&&(S+=e.relevance),N.addSublanguage(e._emitter,e.language)
})():c(),M=""}function u(e,n){let t=1;const i=n.length-1;for(;t<=i;){
if(!e._emit[t]){t++;continue}const i=w.classNameAliases[e[t]]||e[t],r=n[t]
;i?N.addKeyword(r,i):(M=r,c(),M=""),t++}}function h(e,n){
return e.scope&&"string"==typeof e.scope&&N.openNode(w.classNameAliases[e.scope]||e.scope),
e.beginScope&&(e.beginScope._wrap?(N.addKeyword(M,w.classNameAliases[e.beginScope._wrap]||e.beginScope._wrap),
M=""):e.beginScope._multi&&(u(e.beginScope,n),M="")),k=Object.create(e,{parent:{
value:k}}),k}function f(e,n,i){let r=((e,n)=>{const t=e&&e.exec(n)
;return t&&0===t.index})(e.endRe,i);if(r){if(e["on:end"]){const i=new t(e)
;e["on:end"](n,i),i.isMatchIgnored&&(r=!1)}if(r){
for(;e.endsParent&&e.parent;)e=e.parent;return e}}
if(e.endsWithParent)return f(e.parent,n,i)}function p(e){
return 0===k.matcher.regexIndex?(M+=e[0],1):(R=!0,0)}function b(e){
const t=e[0],i=n.substring(e.index),r=f(k,e,i);if(!r)return ee;const s=k
;k.endScope&&k.endScope._wrap?(g(),
N.addKeyword(t,k.endScope._wrap)):k.endScope&&k.endScope._multi?(g(),
u(k.endScope,e)):s.skip?M+=t:(s.returnEnd||s.excludeEnd||(M+=t),
g(),s.excludeEnd&&(M=t));do{
k.scope&&N.closeNode(),k.skip||k.subLanguage||(S+=k.relevance),k=k.parent
}while(k!==r.parent);return r.starts&&h(r.starts,e),s.returnEnd?0:t.length}
let m={};function y(i,s){const o=s&&s[0];if(M+=i,null==o)return g(),0
;if("begin"===m.type&&"end"===s.type&&m.index===s.index&&""===o){
if(M+=n.slice(s.index,s.index+1),!a){const n=Error(`0 width match regex (${e})`)
;throw n.languageName=e,n.badRule=m.rule,n}return 1}
if(m=s,"begin"===s.type)return(e=>{
const n=e[0],i=e.rule,r=new t(i),s=[i.__beforeBegin,i["on:begin"]]
;for(const t of s)if(t&&(t(e,r),r.isMatchIgnored))return p(n)
;return i.skip?M+=n:(i.excludeBegin&&(M+=n),
g(),i.returnBegin||i.excludeBegin||(M=n)),h(i,e),i.returnBegin?0:n.length})(s)
;if("illegal"===s.type&&!r){
const e=Error('Illegal lexeme "'+o+'" for mode "'+(k.scope||"<unnamed>")+'"')
;throw e.mode=k,e}if("end"===s.type){const e=b(s);if(e!==ee)return e}
if("illegal"===s.type&&""===o)return 1
;if(j>1e5&&j>3*s.index)throw Error("potential infinite loop, way more iterations than matches")
;return M+=o,o.length}const w=x(e)
;if(!w)throw K(o.replace("{}",e)),Error('Unknown language: "'+e+'"')
;const E=W(w);let v="",k=s||E;const O={},N=new d.__emitter(d);(()=>{const e=[]
;for(let n=k;n!==w;n=n.parent)n.scope&&e.unshift(n.scope)
;e.forEach((e=>N.openNode(e)))})();let M="",S=0,A=0,j=0,R=!1;try{
for(k.matcher.considerAll();;){
j++,R?R=!1:k.matcher.considerAll(),k.matcher.lastIndex=A
;const e=k.matcher.exec(n);if(!e)break;const t=y(n.substring(A,e.index),e)
;A=e.index+t}
return y(n.substring(A)),N.closeAllNodes(),N.finalize(),v=N.toHTML(),{
language:e,value:v,relevance:S,illegal:!1,_emitter:N,_top:k}}catch(t){
if(t.message&&t.message.includes("Illegal"))return{language:e,value:J(n),
illegal:!0,relevance:0,_illegalBy:{message:t.message,index:A,
context:n.slice(A-100,A+100),mode:t.mode,resultSoFar:v},_emitter:N};if(a)return{
language:e,value:J(n),illegal:!1,relevance:0,errorRaised:t,_emitter:N,_top:k}
;throw t}}function _(e,n){n=n||d.languages||Object.keys(i);const t=(e=>{
const n={value:J(e),illegal:!1,relevance:0,_top:l,_emitter:new d.__emitter(d)}
;return n._emitter.addText(e),n})(e),r=n.filter(x).filter(k).map((n=>$(n,e,!1)))
;r.unshift(t);const s=r.sort(((e,n)=>{
if(e.relevance!==n.relevance)return n.relevance-e.relevance
;if(e.language&&n.language){if(x(e.language).supersetOf===n.language)return 1
;if(x(n.language).supersetOf===e.language)return-1}return 0})),[a,o]=s,c=a
;return c.secondBest=o,c}function y(e){let n=null;const t=(e=>{
let n=e.className+" ";n+=e.parentNode?e.parentNode.className:""
;const t=d.languageDetectRe.exec(n);if(t){const n=x(t[1])
;return n||(q(o.replace("{}",t[1])),
q("Falling back to no-highlight mode for this block.",e)),n?t[1]:"no-highlight"}
return n.split(/\s+/).find((e=>b(e)||x(e)))})(e);if(b(t))return
;if(O("before:highlightElement",{el:e,language:t
}),e.children.length>0&&(d.ignoreUnescapedHTML||(console.warn("One of your code blocks includes unescaped HTML. This is a potentially serious security risk."),
console.warn("https://github.com/highlightjs/highlight.js/wiki/security"),
console.warn("The element with unescaped HTML:"),
console.warn(e)),d.throwUnescapedHTML))throw new Q("One of your code blocks includes unescaped HTML.",e.innerHTML)
;n=e;const i=n.textContent,s=t?m(i,{language:t,ignoreIllegals:!0}):_(i)
;e.innerHTML=s.value,((e,n,t)=>{const i=n&&r[n]||t
;e.classList.add("hljs"),e.classList.add("language-"+i)
})(e,t,s.language),e.result={language:s.language,re:s.relevance,
relevance:s.relevance},s.secondBest&&(e.secondBest={
language:s.secondBest.language,relevance:s.secondBest.relevance
}),O("after:highlightElement",{el:e,result:s,text:i})}let w=!1;function E(){
"loading"!==document.readyState?document.querySelectorAll(d.cssSelector).forEach(y):w=!0
}function x(e){return e=(e||"").toLowerCase(),i[e]||i[r[e]]}
function v(e,{languageName:n}){"string"==typeof e&&(e=[e]),e.forEach((e=>{
r[e.toLowerCase()]=n}))}function k(e){const n=x(e)
;return n&&!n.disableAutodetect}function O(e,n){const t=e;s.forEach((e=>{
e[t]&&e[t](n)}))}
"undefined"!=typeof window&&window.addEventListener&&window.addEventListener("DOMContentLoaded",(()=>{
w&&E()}),!1),Object.assign(n,{highlight:m,highlightAuto:_,highlightAll:E,
highlightElement:y,
highlightBlock:e=>(X("10.7.0","highlightBlock will be removed entirely in v12.0"),
X("10.7.0","Please use highlightElement now."),y(e)),configure:e=>{d=Y(d,e)},
initHighlighting:()=>{
E(),X("10.6.0","initHighlighting() deprecated.  Use highlightAll() now.")},
initHighlightingOnLoad:()=>{
E(),X("10.6.0","initHighlightingOnLoad() deprecated.  Use highlightAll() now.")
},registerLanguage:(e,t)=>{let r=null;try{r=t(n)}catch(n){
if(K("Language definition for '{}' could not be registered.".replace("{}",e)),
!a)throw n;K(n),r=l}
r.name||(r.name=e),i[e]=r,r.rawDefinition=t.bind(null,n),r.aliases&&v(r.aliases,{
languageName:e})},unregisterLanguage:e=>{delete i[e]
;for(const n of Object.keys(r))r[n]===e&&delete r[n]},
listLanguages:()=>Object.keys(i),getLanguage:x,registerAliases:v,
autoDetection:k,inherit:Y,addPlugin:e=>{(e=>{
e["before:highlightBlock"]&&!e["before:highlightElement"]&&(e["before:highlightElement"]=n=>{
e["before:highlightBlock"](Object.assign({block:n.el},n))
}),e["after:highlightBlock"]&&!e["after:highlightElement"]&&(e["after:highlightElement"]=n=>{
e["after:highlightBlock"](Object.assign({block:n.el},n))})})(e),s.push(e)}
}),n.debugMode=()=>{a=!1},n.safeMode=()=>{a=!0
},n.versionString="11.7.0",n.regex={concat:f,lookahead:g,either:p,optional:h,
anyNumberOfTimes:u};for(const n in j)"object"==typeof j[n]&&e.exports(j[n])
;return Object.assign(n,j),n})({}),te=Object.freeze({__proto__:null,
grmr_veryl:e=>({name:"Veryl",aliases:["veryl"],case_insensitive:!1,keywords:{
keyword:"module interface function modport package enum struct parameter localparam posedge negedge async_high async_low sync_high sync_low always_ff always_comb assign return as var inst import export logic bit tri signed u32 u64 i32 i64 f32 f64 input output inout ref if if_reset else for in case for in step repeat initial final inside outside default pub",
literal:""},
contains:[e.QUOTE_STRING_MODE,e.C_BLOCK_COMMENT_MODE,e.C_LINE_COMMENT_MODE,{
scope:"number",contains:[e.BACKSLASH_ESCAPE],variants:[{
begin:/\b((\d+'([bhodBHOD]))[0-9xzXZa-fA-F_]+)/},{
begin:/\B(('([bhodBHOD]))[0-9xzXZa-fA-F_]+)/},{begin:/\b[0-9][0-9_]*/,
relevance:0}]}]}),grmr_verilog:e=>{
const n=e.regex,t=["begin_keywords","celldefine","default_nettype","default_decay_time","default_trireg_strength","define","delay_mode_distributed","delay_mode_path","delay_mode_unit","delay_mode_zero","else","elsif","end_keywords","endcelldefine","endif","ifdef","ifndef","include","line","nounconnected_drive","pragma","resetall","timescale","unconnected_drive","undef","undefineall"]
;return{name:"Verilog",aliases:["v","sv","svh"],case_insensitive:!1,keywords:{
$pattern:/\$?[\w]+(\$[\w]+)*/,
keyword:["accept_on","alias","always","always_comb","always_ff","always_latch","and","assert","assign","assume","automatic","before","begin","bind","bins","binsof","bit","break","buf|0","bufif0","bufif1","byte","case","casex","casez","cell","chandle","checker","class","clocking","cmos","config","const","constraint","context","continue","cover","covergroup","coverpoint","cross","deassign","default","defparam","design","disable","dist","do","edge","else","end","endcase","endchecker","endclass","endclocking","endconfig","endfunction","endgenerate","endgroup","endinterface","endmodule","endpackage","endprimitive","endprogram","endproperty","endspecify","endsequence","endtable","endtask","enum","event","eventually","expect","export","extends","extern","final","first_match","for","force","foreach","forever","fork","forkjoin","function","generate|5","genvar","global","highz0","highz1","if","iff","ifnone","ignore_bins","illegal_bins","implements","implies","import","incdir","include","initial","inout","input","inside","instance","int","integer","interconnect","interface","intersect","join","join_any","join_none","large","let","liblist","library","local","localparam","logic","longint","macromodule","matches","medium","modport","module","nand","negedge","nettype","new","nexttime","nmos","nor","noshowcancelled","not","notif0","notif1","or","output","package","packed","parameter","pmos","posedge","primitive","priority","program","property","protected","pull0","pull1","pulldown","pullup","pulsestyle_ondetect","pulsestyle_onevent","pure","rand","randc","randcase","randsequence","rcmos","real","realtime","ref","reg","reject_on","release","repeat","restrict","return","rnmos","rpmos","rtran","rtranif0","rtranif1","s_always","s_eventually","s_nexttime","s_until","s_until_with","scalared","sequence","shortint","shortreal","showcancelled","signed","small","soft","solve","specify","specparam","static","string","strong","strong0","strong1","struct","super","supply0","supply1","sync_accept_on","sync_reject_on","table","tagged","task","this","throughout","time","timeprecision","timeunit","tran","tranif0","tranif1","tri","tri0","tri1","triand","trior","trireg","type","typedef","union","unique","unique0","unsigned","until","until_with","untyped","use","uwire","var","vectored","virtual","void","wait","wait_order","wand","weak","weak0","weak1","while","wildcard","wire","with","within","wor","xnor","xor"],
literal:["null"],
built_in:["$finish","$stop","$exit","$fatal","$error","$warning","$info","$realtime","$time","$printtimescale","$bitstoreal","$bitstoshortreal","$itor","$signed","$cast","$bits","$stime","$timeformat","$realtobits","$shortrealtobits","$rtoi","$unsigned","$asserton","$assertkill","$assertpasson","$assertfailon","$assertnonvacuouson","$assertoff","$assertcontrol","$assertpassoff","$assertfailoff","$assertvacuousoff","$isunbounded","$sampled","$fell","$changed","$past_gclk","$fell_gclk","$changed_gclk","$rising_gclk","$steady_gclk","$coverage_control","$coverage_get","$coverage_save","$set_coverage_db_name","$rose","$stable","$past","$rose_gclk","$stable_gclk","$future_gclk","$falling_gclk","$changing_gclk","$display","$coverage_get_max","$coverage_merge","$get_coverage","$load_coverage_db","$typename","$unpacked_dimensions","$left","$low","$increment","$clog2","$ln","$log10","$exp","$sqrt","$pow","$floor","$ceil","$sin","$cos","$tan","$countbits","$onehot","$isunknown","$fatal","$warning","$dimensions","$right","$high","$size","$asin","$acos","$atan","$atan2","$hypot","$sinh","$cosh","$tanh","$asinh","$acosh","$atanh","$countones","$onehot0","$error","$info","$random","$dist_chi_square","$dist_erlang","$dist_exponential","$dist_normal","$dist_poisson","$dist_t","$dist_uniform","$q_initialize","$q_remove","$q_exam","$async$and$array","$async$nand$array","$async$or$array","$async$nor$array","$sync$and$array","$sync$nand$array","$sync$or$array","$sync$nor$array","$q_add","$q_full","$psprintf","$async$and$plane","$async$nand$plane","$async$or$plane","$async$nor$plane","$sync$and$plane","$sync$nand$plane","$sync$or$plane","$sync$nor$plane","$system","$display","$displayb","$displayh","$displayo","$strobe","$strobeb","$strobeh","$strobeo","$write","$readmemb","$readmemh","$writememh","$value$plusargs","$dumpvars","$dumpon","$dumplimit","$dumpports","$dumpportson","$dumpportslimit","$writeb","$writeh","$writeo","$monitor","$monitorb","$monitorh","$monitoro","$writememb","$dumpfile","$dumpoff","$dumpall","$dumpflush","$dumpportsoff","$dumpportsall","$dumpportsflush","$fclose","$fdisplay","$fdisplayb","$fdisplayh","$fdisplayo","$fstrobe","$fstrobeb","$fstrobeh","$fstrobeo","$swrite","$swriteb","$swriteh","$swriteo","$fscanf","$fread","$fseek","$fflush","$feof","$fopen","$fwrite","$fwriteb","$fwriteh","$fwriteo","$fmonitor","$fmonitorb","$fmonitorh","$fmonitoro","$sformat","$sformatf","$fgetc","$ungetc","$fgets","$sscanf","$rewind","$ftell","$ferror"]
},contains:[e.C_BLOCK_COMMENT_MODE,e.C_LINE_COMMENT_MODE,e.QUOTE_STRING_MODE,{
scope:"number",contains:[e.BACKSLASH_ESCAPE],variants:[{
begin:/\b((\d+'([bhodBHOD]))[0-9xzXZa-fA-F_]+)/},{
begin:/\B(('([bhodBHOD]))[0-9xzXZa-fA-F_]+)/},{begin:/\b[0-9][0-9_]*/,
relevance:0}]},{scope:"variable",variants:[{begin:"#\\((?!parameter).+\\)"},{
begin:"\\.\\w+",relevance:0}]},{scope:"variable.constant",
match:n.concat(/`/,n.either("__FILE__","__LINE__"))},{scope:"meta",
begin:n.concat(/`/,n.either(...t)),end:/$|\/\/|\/\*/,returnEnd:!0,keywords:t}]}
},grmr_ini:e=>{const n=e.regex,t={className:"number",relevance:0,variants:[{
begin:/([+-]+)?[\d]+_[\d_]+/},{begin:e.NUMBER_RE}]},i=e.COMMENT();i.variants=[{
begin:/;/,end:/$/},{begin:/#/,end:/$/}];const r={className:"variable",
variants:[{begin:/\$[\w\d"][\w\d_]*/},{begin:/\$\{(.*?)\}/}]},s={
className:"literal",begin:/\bon|off|true|false|yes|no\b/},a={className:"string",
contains:[e.BACKSLASH_ESCAPE],variants:[{begin:"'''",end:"'''",relevance:10},{
begin:'"""',end:'"""',relevance:10},{begin:'"',end:'"'},{begin:"'",end:"'"}]
},o={begin:/\[/,end:/\]/,contains:[i,s,r,a,t,"self"],relevance:0
},l=n.either(/[A-Za-z0-9_-]+/,/"(\\"|[^"])*"/,/'[^']*'/);return{
name:"TOML, also INI",aliases:["toml"],case_insensitive:!0,illegal:/\S/,
contains:[i,{className:"section",begin:/\[+/,end:/\]+/},{
begin:n.concat(l,"(\\s*\\.\\s*",l,")*",n.lookahead(/\s*=\s*[^#\s]/)),
className:"attr",starts:{end:/$/,contains:[i,o,s,r,a,t]}}]}}});const ie=ne
;for(const e of Object.keys(te)){const n=e.replace("grmr_","").replace("_","-")
;ie.registerLanguage(n,te[e])}return ie}()
;"object"==typeof exports&&"undefined"!=typeof module&&(module.exports=hljs);