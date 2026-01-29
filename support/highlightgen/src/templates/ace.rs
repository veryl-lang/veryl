use crate::templates::Template;
use handlebars::Handlebars;
use std::path::PathBuf;

const TMPL: &str = r###"ace.define("ace/mode/veryl_highlight_rules",["require","exports","module","ace/lib/oop","ace/mode/text_highlight_rules"], function(require, exports, module){"use strict";
var oop = require("../lib/oop");
var TextHighlightRules = require("./text_highlight_rules").TextHighlightRules;
var VerylHighlightRules = function () {
    var keywords = "{{#each this}}{{#each this}}{{{this}}}{{#unless @last}}|{{/unless}}{{/each}}{{#unless @last}}|{{/unless}}{{/each}}";
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
"###;

pub struct Ace;

impl Template for Ace {
    fn apply(&self, keywords: &crate::keywords::Keywords) -> String {
        let mut handlebars = Handlebars::new();
        handlebars.register_escape_fn(handlebars::no_escape);
        handlebars.render_template(TMPL, &keywords).unwrap()
    }

    fn path(&self) -> PathBuf {
        PathBuf::from("doc/book/mode-veryl.js")
    }
}
