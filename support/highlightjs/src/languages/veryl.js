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
        keyword: 'module interface function modport package enum struct param local clock clock_posedge clock_negedge reset reset_async_high reset_async_low reset_sync_high reset_sync_low always_ff always_comb assign return as var inst import export logic bit tri signed u32 u64 i32 i64 f32 f64 input output inout ref if if_reset else for in case switch step repeat initial final inside outside default pub let break embed include unsafe',
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
