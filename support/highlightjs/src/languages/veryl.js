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
        'veryl'
    ],
    case_insensitive: false,
    keywords:
      {
        keyword: 'module interface function modport package enum struct param local clock clock_posedge clock_negedge reset reset_async_high reset_async_low reset_sync_high reset_sync_low always_ff always_comb assign return as var inst import logic bit tri signed u8 u16 u32 u64 i8 i16 i32 i64 f32 f64 bool true false input output inout if if_reset else for in rev case switch step repeat initial final inside outside default pub let break embed include unsafe type const alias proto converse same',
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
