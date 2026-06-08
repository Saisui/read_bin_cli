# require 'tempfile'

# require_relative 'cli/puella-cli-ruby/lib/kolor_more'

bin = File.read ARGV[0]

packs = bin.bytes.each_slice(4096).to_a

packs.each_with_index do |bin, pack_i|

  shows = []

  bin.each do |b|
    case b
    when 0
      shows << "\e[31m. \e[0m"
    when 0xd
      shows << "\e[44m\\r\e[0m"
    when 10
      # shows << "\e[44m\\n\e[0m"
      shows << "⏎ "
    when 0x1b
      shows << "\e[43m\\e\e[0m"
    when 0x0..0x1f
      shows << "\e[41m%02x\e[0m" % [b]
    when 0x20
      shows << "· "
    when 0x21..0x7e
      shows << ("%s " % b.chr)
      # shows << b.chr + "\e[4;3#{b/16-1}m#{("%02x"%[b])[1]}\e[0m"
  #
    when 0b10_000000..0b10_111111
  #
      # shows << ("\e[44m%02x\e[0m" % [b])
      ch = "%02x" % [b]
      ch1, ch2 = ch.chars
      ch1 = "\e[48;2;0;100;255m#{ch1}\e[0m"
      ch2 = "\e[48;2;0;120;255m#{ch2}\e[0m"
      shows << ch1 + ch2
    else
      ch = "%02x" % [b]
      ch1, ch2 = ch.chars
      ch1 = "\e[48;2;100;0;255m#{ch1}\e[0m"
      ch2 = "\e[48;2;120;0;255m#{ch2}\e[0m"
      shows << ch1 + ch2
      # shows << "\e[45m%02x\e[0m" % [b]
    end
  end

  last = ""

  last << "pack: #{pack_i+1} / #{packs.size}\n"

  last << "\e[44m   0 1 2 3 4 5 6 7 8 9 a b c d e f \e[0m\n"
  shows.each_slice(16).each_with_index do |ss, idx|
    last << "%02x " % idx
    ss.each do |b|
      last << b
    end
    last << "\n"
  end

  begin
    IO.popen "less -R", "w" do |c|
      c.puts last
    end
  rescue => err
  rescue Interrupt
    exit
  end
end
