def popcount(n)
  n.digits(2).sum
end

def candidate_columns(check_width)
  (1...(2**check_width))
    .map { |i| [i, popcount(i)] }
    .select { |(_, weight)| weight.odd? && weight >= 3 }
    .sort_by { |(v, weight)| [weight, v] }
end

def build_h_columuns(data_width, check_width)
  candidates = candidate_columns(check_width)

  row_bits = Array.new(check_width, 0)
  selected = []

  data_width.times do
    min_weight = candidates.map { |(_, weight)| weight }.min
    pool = candidates.select { |(_, weight)| weight == min_weight }

    best, _ = pool.min_by do |(v, _)|
      after = row_bits.dup
      check_width.times { |i| after[i] += v[i] }
      [after.max, after.sum, v]
    end

    selected << best
    check_width.times { |i| row_bits[i] += best[i] }
    candidates.delete_if { |(v, _)| v == best }
  end

  selected
end

def build_h_matrix(data_width, check_width)
  columns = build_h_columuns(data_width, check_width)
  Array.new(check_width) do |row|
    bits = 1 << row
    columns.each_with_index { |column_bits, column| bits |= column_bits[row] << (column + check_width) }
    bits
  end
end

def h_matrix_comment(rows, data_width, check_width)
  lines = []
  lines << "    // data width = #{data_width} check width = #{check_width}"
  rows
    .map.with_index { |bits, i| [bits, i] }
    .reverse_each do |bits, i|
      data_part = format('%0*b', data_width, bits[check_width...])
      check_part = format('%0*b', check_width, bits[0...check_width])
      lines << "    // row[#{i}]: #{data_part} | #{check_part}"
    end
  lines.join("\n")
end

# Minimum number of check bits for a SEC-DED code with the given data width.
# The usable columns are odd-weight vectors of length m excluding the m weight-1
# ones, i.e. 2^(m-1) - m must be at least the data width.
def min_check_width(data_width)
  m = 2
  m += 1 while (2**(m - 1) - m) < data_width
  m
end

# Generate a HEX literal of the given bit width.
def hex_literal(value, width)
  digits = (width + 3) / 4
  format("%d'h%0*x", width, digits, value)
end

# H column pattern for data bit j (row i as bit i): the single-error syndrome.
def data_column_values(h_matrix, data_width, check_width)
  n = data_width + check_width
  (0...n).map do |column|
    value = 0
    h_matrix.each_with_index { |row_bits, row| value |= row_bits[column] << row }
    value
  end
end

def emit_consts(data_width, check_width)
  lines = []
  lines << "    const DATA_WIDTH : u32 = #{data_width};"
  lines << "    const CHECK_WIDTH: u32 = #{check_width};"
  lines << "    const CODE_WIDTH : u32 = DATA_WIDTH + CHECK_WIDTH;"
  lines.join("\n")
end

def emit_encode_matrix(h_matrix, data_width, check_width)
  lines = []
  lines << "    // Coefficient matrix for check-bit generation."
  h_matrix.each_with_index do |row, i|
    value = hex_literal(row >> check_width, data_width)
    lines << "    const ENCODE_MATRIX_ROW_#{i}: bit<DATA_WIDTH> = #{value}; // check[#{i}]"
  end
  lines.join("\n")
end

def emit_struct
  lines = []
  lines << "    struct decode_result {"
  lines << "        error_pos: logic<CODE_WIDTH>, // error position (one-hot, [CHECK_WIDTH-1:0]=check, upper=data)"
  lines << "        corrected: logic            , // a single-bit error was located and corrected"
  lines << "        detected : logic            , // an uncorrectable error was detected"
  lines << "    }"
  lines.join("\n")
end

# Function that computes the check bits.
def emit_encode(check_width)
  lines = []
  lines << "    /// Compute the check bits (AND with the coefficient matrix, then XOR reduction)."
  lines << "    function encode ("
  lines << "        data: input logic<DATA_WIDTH>,"
  lines << "    ) -> logic<CHECK_WIDTH> {"
  lines << "        var check: logic<CHECK_WIDTH>;"
  check_width.times do |row|
    lines << "        check[#{row}] = ^(ENCODE_MATRIX_ROW_#{row} & data);"
  end
  lines << "        return check;"
  lines << "    }"
  lines.join("\n")
end

# Function that computes the syndrome.
def emit_syndrome
  lines = []
  lines << "    /// Compute the syndrome (received check bits ^ recomputed check bits)."
  lines << "    function calc_syndrome ("
  lines << "        code: input logic<CODE_WIDTH>"
  lines << "    ) -> logic<CHECK_WIDTH> {"
  lines << "        var data : logic<DATA_WIDTH> ;"
  lines << "        var check: logic<CHECK_WIDTH>;"
  lines << "        data  = code[CODE_WIDTH-1-:DATA_WIDTH];"
  lines << "        check = code[0+:CHECK_WIDTH];"
  lines << "        return encode(data) ^ check;"
  lines << "    }"
  lines.join("\n")
end

# Function that locates the error position and detects errors from the syndrome.
def emit_decode(h_matrix, data_width, check_width)
  columns = data_column_values(h_matrix, data_width, check_width)
  code_width = data_width + check_width

  # [condition, error position (one-hot), comment, corrected]
  entries = []
  entries << ["syndrome == #{hex_literal(0, check_width)}", 0, "no error", false]
  columns.each_with_index do |column_bits, i|
    condition, comment =
      if i < check_width
        ["syndrome == #{hex_literal(column_bits, check_width)}", "check[#{i}]"]
      else
        pos = i - check_width
        w = pos + 1
        ["syndrome == #{hex_literal(column_bits, check_width)} && width >= #{w}", "data[#{pos}]"]
      end
    entries << [condition, 1 << i, comment, true]
  end

  lines = []
  lines << "    /// Determine the error position and flags (corrected/detected) from the syndrome."
  lines << "    /// A data-bit correction is honored only while its index is below the active width."
  lines << "    function decode ("
  lines << "        syndrome: input logic<CHECK_WIDTH>,"
  lines << "        width   : input u32               ,"
  lines << "    ) -> decode_result {"
  lines << "        // Boolean-selector switch (SV: case (1'b1)); the syndrome values are mutually"
  lines << "        // exclusive, so the branches stay parallel while sharing the width gate."
  lines << "        // A data branch that fails its width guard falls through to default -> detected."
  lines << "        switch {"
  entries.each do |condition, error_pos, comment, corrected|
    lines << "            #{condition}: return decode_result'{error_pos: #{hex_literal(error_pos, code_width)}, corrected: #{corrected}, detected: false}; // #{comment}"
  end
  lines << "            default: return decode_result'{error_pos: #{hex_literal(0, code_width)}, corrected: false, detected: true}; // undefined syndrome / out-of-range (multiple-bit error)"
  lines << "        }"
  lines << "    }"
  lines.join("\n")
end

def build_veryl(h_matrix, data_width, check_width)
  header = "#[fmt(skip)]\npub package hamming_#{data_width}_#{check_width}_pkg {\n" +
           h_matrix_comment(h_matrix, data_width, check_width)

  sections = [
    header,
    emit_consts(data_width, check_width),
    emit_encode_matrix(h_matrix, data_width, check_width),
    emit_struct,
    emit_encode(check_width),
    emit_syndrome,
    emit_decode(h_matrix, data_width, check_width),
  ]

  sections.join("\n\n") + "\n}\n"
end

# Generate a Veryl package for the given data / check widths and write it to a file.
def generate_veryl(data_width, check_width)
  h_matrix = build_h_matrix(data_width, check_width)
  veryl = build_veryl(h_matrix, data_width, check_width)
  path = "hamming_#{data_width}_#{check_width}_pkg.veryl"
  File.write(path, veryl)
  puts "generated: #{path}"
end

# Maximum data width supported by a SEC-DED code with the given check width:
# all odd-weight vectors of length m (2^(m-1)) minus the m weight-1 columns.
def max_data_width(check_width)
  2**(check_width - 1) - check_width
end

# Scan by check (code) width; generate each package at its maximum data width.
(5..9).each do |check_width|
  generate_veryl(max_data_width(check_width), check_width)
end
