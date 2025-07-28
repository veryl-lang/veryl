use crate::templates::Template;
use handlebars::Handlebars;
use std::path::PathBuf;

const TMPL: &str = r###"# -*- coding: utf-8 -*- #
# frozen_string_literal: true

module Rouge
  module Lexers
    class Veryl < RegexLexer
      title "Veryl"
      desc "The Veryl hardware description language (https://veryl-lang.org)"
      tag 'veryl'
      filenames '*.veryl'
      mimetypes 'text/x-veryl'

      # Characters

      WHITE_SPACE = /\s+/
      NEWLINE     = /\n/

      # Comments

      LINE_COMMENT    = /\/\/(?:(?!#{NEWLINE}).)*/
      GENERAL_COMMENT = /\/\*(?:(?!\*\/).)*\*\//m
      COMMENT         = /#{LINE_COMMENT}|#{GENERAL_COMMENT}/

      # Numeric literals

      EXPONENT    = /[0-9]+(?:_[0-9]+)*\.[0-9]+(?:_[0-9]+)*[eE][+-]?[0-9]+(?:_[0-9]+)*/
      FIXED_POINT = /[0-9]+(?:_[0-9]+)*\.[0-9]+(?:_[0-9]+)*/
      BASED       = /(?:[0-9]+(?:_[0-9]+)*)?'s?[bodh][0-9a-fA-FxzXZ]+(?:_[0-9a-fA-FxzXZ]+)*/
      ALL_BIT     = /(?:[0-9]+(?:_[0-9]+)*)?'[01xzXZ]/
      BASE_LESS   = /[0-9]+(?:_[0-9]+)*/
 
      # Operators and delimiters

      OPERATOR  = / -:     | ->     | \+:    | \+=    | -=
                  | \*=    | \/=    | %=     | &=     | \|=
                  | \^=    | <<=    | >>=    |<<<=    |>>>=
                  | <>     | \*\*   | \/     | \|     | %
                  | \+     | -      | <<<    | >>>    | <<
                  | >>     | <=     | >=     | <:     | >:
                  | ===    | ==\?   | \!==   | \!=\?  | ==
                  | \!=    | &&     | \|\|   | &      | \^~
                  | \^     | ~\^    | \|     | ~&     | ~\|
                  | \!     | ~
                  /x

      SEPARATOR = / ::<    | ::     | :      | ,      | \.\.=
                  | \.\.   | \.     | =      | \#     | <
                  | \?     | '      | '\{    | \{     | \[
                  | \(     | >      | \}     | \]     | \)
                  | ;      | \*
                  /x

      # Identifiers

      DOLLAR_IDENTIFIER = /\$[a-zA-Z_][0-9a-zA-Z_$]*/
      IDENTIFIER        = /(?:r#)?[a-zA-Z_][0-9a-zA-Z_$]*/

      # Keywords

      def self.keywords
        @keywords ||= Set.new %w(
          {{#each structure}}{{{this}}}{{#unless @last}} {{/unless}}{{/each}}
          {{#each statement}}{{{this}}}{{#unless @last}} {{/unless}}{{/each}}
          {{#each direction}}{{{this}}}{{#unless @last}} {{/unless}}{{/each}}
          {{#each conditional}}{{{this}}}{{#unless @last}} {{/unless}}{{/each}}
          {{#each repeat}}{{{this}}}{{#unless @last}} {{/unless}}{{/each}}
        )
      end

      def self.keywords_type
        @keywords_type ||= Set.new %w(
          {{#each type}}{{{this}}}{{#unless @last}} {{/unless}}{{/each}}
        )
      end

      state :root do
        rule(COMMENT          , Comment             )
        rule(EXPONENT         , Num::Float          )
        rule(FIXED_POINT      , Num::Float          )
        rule(BASED            , Num::Integer        )
        rule(ALL_BIT          , Num::Integer        )
        rule(BASE_LESS        , Num::Integer        )
        rule(OPERATOR         , Operator            )
        rule(SEPARATOR        , Punctuation         )
        rule(DOLLAR_IDENTIFIER, Name                )
        rule(WHITE_SPACE      , Text                )
        rule(/"/              , Str::Double, :string)

        rule IDENTIFIER do |m|
          name = m[0]

          if self.class.keywords.include? name
            token Keyword
          elsif self.class.keywords_type.include? name
            token Keyword::Type
          else
            token Name
          end
        end
      end

      state :string do
        rule(/[^\\"]+/, Str::Double       )
        rule(/\\./    , Str::Escape       )
        rule(/"/      , Str::Double, :pop!)
      end
    end
  end
end
"###;

pub struct Rouge;

impl Template for Rouge {
    fn apply(&self, keywords: &crate::keywords::Keywords) -> String {
        let mut handlebars = Handlebars::new();
        handlebars.register_escape_fn(handlebars::no_escape);
        handlebars.render_template(TMPL, &keywords).unwrap()
    }

    fn path(&self) -> PathBuf {
        PathBuf::from("support/rouge/lib/rouge/lexers/veryl.rb")
    }
}
