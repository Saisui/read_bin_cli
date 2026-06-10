module UTF8
  module_functions
  def at hex
    case hex
    when 0..0x7f
      :ascii
    when 0x80..0xbf
      :tail
    when 0xc0,0xc1
      :invalid
    when 0xc2..0xdf
      :duo
    when 0xe0..0xef
      :trio
    when 0b1111_0000..0b1111_0111
      :quo
    when 0xc2..0xf7
      :head
    end
  end
end
