package basalt.oracle;

import com.google.gson.JsonObject;
import io.netty.buffer.ByteBuf;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelOutboundHandlerAdapter;
import io.netty.channel.ChannelPromise;
import net.minecraft.network.protocol.Packet;

import java.io.BufferedWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.zip.Inflater;

/**
 * Serverbound packet log (JSONL). ConnectionMixin notes (tick, class name) for every packet handed
 * to Connection.send; a netty handler placed next to the packet encoder sees the encoded bytes
 * (packet id varint + body, decompressed if the connection compresses) in the same order and writes
 * one line per packet: {"tick","seq","wall_ms","name","hex"}. Marks from the control channel are
 * interleaved as {"tick","mark"}. Test-only; everything here only calls public APIs.
 */
public final class PacketLog {
    private PacketLog() {}

    private static final String HANDLER = "basalt_oracle_log";
    private static final String PRE = "basalt_oracle_pre";
    private static final java.util.Map<Object, Long> STAMPS = java.util.Collections.synchronizedMap(new java.util.IdentityHashMap<>());
    private static final ConcurrentLinkedQueue<String[]> PENDING = new ConcurrentLinkedQueue<>();
    private static BufferedWriter out;
    private static Path path;
    private static long seq;
    private static final long T0 = System.currentTimeMillis();

    /** (Re)open the log file; truncates. */
    public static synchronized void open(Path p) throws Exception {
        close();
        path = p;
        if (p.getParent() != null) Files.createDirectories(p.getParent());
        out = Files.newBufferedWriter(p, StandardCharsets.UTF_8);
        seq = 0;
        PENDING.clear();
    }

    public static synchronized void close() {
        try { if (out != null) { out.flush(); out.close(); } } catch (Throwable ignored) {}
        out = null;
    }

    public static synchronized void flush() { try { if (out != null) out.flush(); } catch (Throwable ignored) {} }

    public static synchronized long count() { return seq; }
    public static synchronized String file() { return path == null ? null : path.toString(); }

    public static synchronized void mark(String label) {
        if (out == null) return;
        JsonObject o = new JsonObject();
        o.addProperty("tick", Control.tick);
        o.addProperty("mark", label);
        line(o);
    }

    /** A locally made effect (particle / sound) from the fx mixins, stamped with the client tick. */
    public static synchronized void fx(JsonObject o) {
        if (out == null) return;
        o.addProperty("tick", Control.tick);
        line(o);
    }

    private static void line(JsonObject o) {
        try { out.write(o.toString()); out.write('\n'); out.flush(); } catch (Throwable ignored) {}
    }

    /** Called from ConnectionMixin on the sending thread (the client main thread for gameplay packets). */
    public static void onSend(Channel ch, Packet<?> p) {
        if (ch == null || out == null) return;
        try {
            if (ch.pipeline().get(HANDLER) == null) {
                String enc = ch.pipeline().get("encoder") != null ? "encoder" : null;
                if (enc == null) return;
                ch.pipeline().addBefore(enc, HANDLER, new Sniffer(ch));
                ch.pipeline().addAfter(enc, PRE, new PreSniffer());
                synchronized (PacketLog.class) {
                    JsonObject o = new JsonObject();
                    o.addProperty("tick", Control.tick);
                    o.addProperty("mark", "pipeline " + String.join(",", ch.pipeline().names()));
                    if (out != null) line(o);
                }
            }
            STAMPS.put(p, Control.tick);
        } catch (Throwable ignored) {}
    }

    /** Sees the Packet objects (same event-loop order as the bytes the Sniffer sees): pairs tick + class name. */
    private static final class PreSniffer extends ChannelOutboundHandlerAdapter {
        @Override public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
            try {
                if (msg instanceof Packet<?>) {
                    Long t = STAMPS.remove(msg);
                    String n = msg.getClass().getName();
                    n = n.substring(n.lastIndexOf('.') + 1);
                    PENDING.add(new String[] { Long.toString(t == null ? -1L : t), n });
                }
            } catch (Throwable ignored) {}
            super.write(ctx, msg, promise);
        }
    }

    private static final class Sniffer extends ChannelOutboundHandlerAdapter {
        final Channel ch;
        Sniffer(Channel ch) { this.ch = ch; }

        @Override public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
            try {
                if (msg instanceof ByteBuf bb && out != null) record(bb);
            } catch (Throwable ignored) {}
            super.write(ctx, msg, promise);
        }

        void record(ByteBuf bb) {
            byte[] raw = new byte[bb.readableBytes()];
            bb.getBytes(bb.readerIndex(), raw);
            byte[] data = raw;
            java.util.List<String> names = ch.pipeline().names();
            // outbound traversal runs from the tail to the head: a compress handler with a higher
            // list index than ours has already framed the bytes we see
            if (names.indexOf("compress") > names.indexOf(HANDLER)) {
                // compressed framing: varint dataLength (0 = stored raw) then the payload
                int i = 0, len = 0, shift = 0;
                while (i < raw.length) { int b = raw[i++]; len |= (b & 0x7f) << shift; shift += 7; if ((b & 0x80) == 0) break; }
                try {
                    if (len == 0) data = java.util.Arrays.copyOfRange(raw, i, raw.length);
                    else {
                        Inflater inf = new Inflater();
                        inf.setInput(raw, i, raw.length - i);
                        data = new byte[len];
                        inf.inflate(data);
                        inf.end();
                    }
                } catch (Throwable e) { data = raw; }
            }
            String[] meta = PENDING.poll();
            StringBuilder hex = new StringBuilder(data.length * 2);
            for (byte b : data) { hex.append(Character.forDigit((b >> 4) & 15, 16)).append(Character.forDigit(b & 15, 16)); }
            synchronized (PacketLog.class) {
                if (out == null) return;
                JsonObject o = new JsonObject();
                o.addProperty("tick", meta == null ? -1L : Long.parseLong(meta[0]));
                o.addProperty("seq", seq++);
                o.addProperty("wall_ms", System.currentTimeMillis() - T0);
                o.addProperty("name", meta == null ? "?" : meta[1]);
                o.addProperty("hex", hex.toString());
                line(o);
            }
        }
    }
}
