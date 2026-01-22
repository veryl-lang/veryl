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
        keyword: 'case default else if_reset if inside outside switch converse inout input output same false lsb msb true for in repeat rev step alias always_comb always_ff assign as bind block connect const final import initial inst let param return break type var embed enum function include interface modport module package proto pub struct union unsafe bit bool clock clock_posedge clock_negedge f32 f64 i8 i16 i32 i64 logic reset reset_async_high reset_async_low reset_sync_high reset_sync_low signed string tri u8 u16 u32 u64',
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
