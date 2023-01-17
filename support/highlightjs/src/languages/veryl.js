/**
 * Language: Veryl
 * Contributors:
 *   Naoya Hatta <dalance@gmail.com>
 */
module.exports = function (hljs)
{
  return {
    name: 'Veryl',
    aliases: [
        'vl'
    ],
    case_insensitive: false,
    keywords:
      {
        keyword: 'module interface function modport package enum struct parameter localparam posedge negedge async_high async_low sync_high sync_low always_ff always_comb assign return as var inst import export logic bit tri signed u32 u64 i32 i64 f32 f64 input output inout ref if if_reset else for in case for in step repeat',
        literal: ''
      },
    contains:
      [
        hljs.QUOTE_STRING_MODE,
        hljs.C_BLOCK_COMMENT_MODE,
        hljs.C_LINE_COMMENT_MODE,
        {
          scope: 'number',
          contains: [ hljs.BACKSLASH_ESCAPE ],
          variants: [
            { begin: /\b((\d+'([bhodBHOD]))[0-9xzXZa-fA-F_]+)/ },
            { begin: /\B(('([bhodBHOD]))[0-9xzXZa-fA-F_]+)/ },
            { // decimal
              begin: /\b[0-9][0-9_]*/,
              relevance: 0
            }
          ]
        }
      ]
  }
}
