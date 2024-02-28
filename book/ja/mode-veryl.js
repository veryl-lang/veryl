ace.define("ace/mode/veryl_highlight_rules",["require","exports","module","ace/lib/oop","ace/mode/text_highlight_rules"], function(require, exports, module){"use strict";
var oop = require("../lib/oop");
var TextHighlightRules = require("./text_highlight_rules").TextHighlightRules;
var VerylHighlightRules = function () {
    var keywords = "always_comb|always_ff|as|assign|async_high|async_low|bit|else|enum|export|" +
        "f32|f64|final|for|function|i32|i64|if|if_reset|import|in|initial|inout|input|" +
        "inside|inst|interface|localparam|logic|modport|module|negedge|output|outside|" +
        "package|parameter|posedge|pub|ref|repeat|return|signed|step|struct|sync_high|sync_low|tri|u32|u64|var";
    var keywordMapper = this.createKeywordMapper({
        "keyword": keywords,
    }, "identifier", false);
    this.$rules = {
        "start": [{
                token: "comment",
                regex: "//.*$"
            }, {
                token: "comment.start",
                regex: "/\\*",
                next: [
                    { token: "comment.end", regex: "\\*/", next: "start" },
                    { defaultToken: "comment" }
                ]
            }, {
                token: "string.start",
                regex: '"',
                next: [
                    { token: "constant.language.escape", regex: /\\(?:[ntvfa\\"]|[0-7]{1,3}|\x[a-fA-F\d]{1,2}|)/, consumeLineEnd: true },
                    { token: "string.end", regex: '"|$', next: "start" },
                    { defaultToken: "string" }
                ]
            }, {
                token: "string",
                regex: "'^[']'"
            }, {
                token: "constant.numeric", // float
                regex: "[+-]?\\d+(?:(?:\\.\\d*)?(?:[eE][+-]?\\d+)?)?\\b"
            }, {
                token: keywordMapper,
                regex: "[a-zA-Z_$][a-zA-Z0-9_$]*\\b"
            }, {
                token: "keyword.operator",
                regex: "\\+|\\-|\\/|\\/\\/|%|<@>|@>|<@|&|\\^|~|<|>|<=|=>|==|!=|<>|="
            }, {
                token: "paren.lparen",
                regex: "[\\(]"
            }, {
                token: "paren.rparen",
                regex: "[\\)]"
            }, {
                token: "text",
                regex: "\\s+"
            }]
    };
    this.normalizeRules();
};
oop.inherits(VerylHighlightRules, TextHighlightRules);
exports.VerylHighlightRules = VerylHighlightRules;

});

ace.define("ace/mode/veryl",["require","exports","module","ace/lib/oop","ace/mode/text","ace/mode/veryl_highlight_rules","ace/range"], function(require, exports, module){"use strict";
var oop = require("../lib/oop");
var TextMode = require("./text").Mode;
var VerylHighlightRules = require("./veryl_highlight_rules").VerylHighlightRules;
var Range = require("../range").Range;
var Mode = function () {
    this.HighlightRules = VerylHighlightRules;
    this.$behaviour = this.$defaultBehaviour;
};
oop.inherits(Mode, TextMode);
(function () {
    this.lineCommentStart = "//";
    this.blockComment = { start: "/*", end: "*/" };
    this.$quotes = { '"': '"' };
    this.$id = "ace/mode/veryl";
}).call(Mode.prototype);
exports.Mode = Mode;

});                (function() {
                    ace.require(["ace/mode/veryl"], function(m) {
                        if (typeof module == "object" && typeof exports == "object" && module) {
                            module.exports = m;
                        }
                    });
                })();
            
