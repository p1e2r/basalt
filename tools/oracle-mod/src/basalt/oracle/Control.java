package basalt.oracle;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.mojang.blaze3d.platform.InputConstants;
import net.minecraft.client.KeyMapping;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.screens.ConnectScreen;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.gui.screens.TitleScreen;
import net.minecraft.client.gui.screens.inventory.AbstractContainerScreen;
import net.minecraft.client.input.KeyEvent;
import net.minecraft.client.multiplayer.ServerData;
import net.minecraft.client.multiplayer.resolver.ServerAddress;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.inventory.AbstractContainerMenu;
import net.minecraft.world.inventory.ContainerInput;
import net.minecraft.world.inventory.Slot;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.HitResult;

import java.io.*;
import java.lang.reflect.Method;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.*;
import java.util.concurrent.ConcurrentLinkedQueue;

/**
 * Scripted-input control channel (test-only). Line-based JSON over TCP on 127.0.0.1
 * (-Dbasalt.oracle.ctl.port, default 25789). One request per line, one response per line, in order:
 *   {"id": 1, "op": "move", "forward": true}   ->   {"id": 1, "ok": true, "tick": 1234, ...}
 * Commands are queued and applied at the START of client ticks (Minecraft.tick HEAD), in order; a
 * blocking command (wait, wait_join, ...) holds back the ones behind it until it completes, so a
 * script is tick-exact and every response reports the tick it executed (or completed) on.
 * Everything here calls public client API (signatures found with javap) plus two reflective
 * pokes (the container screen's protected slotClicked, the mouse-grab flag); see SCHEMA.md.
 */
public final class Control {
    private Control() {}

    /** Client ticks since start; incremented at the start of every Minecraft.tick. */
    public static volatile long tick;

    private static final class Cmd { final JsonObject req; final Conn conn; Cmd(JsonObject r, Conn c) { req = r; conn = c; } }

    private static final class Conn {
        final Socket sock; final PrintWriter w;
        Conn(Socket s) throws IOException { sock = s; w = new PrintWriter(new OutputStreamWriter(s.getOutputStream(), StandardCharsets.UTF_8), true); }
        synchronized void send(JsonObject o) { w.println(o.toString()); }
    }

    private static final ConcurrentLinkedQueue<Cmd> QUEUE = new ConcurrentLinkedQueue<>();
    private static final ArrayDeque<Cmd> LOCAL = new ArrayDeque<>();       // main thread only
    private static final List<Object[]> RELEASES = new ArrayList<>();      // {KeyMapping, dueTick}
    private static Cmd blocking;            // the blocking command in progress
    private static long blockUntil;         // wait: completes at this tick
    private static long blockDeadline;      // wait_*: fail at this tick
    private static boolean started;
    private static boolean forceGrab = true;
    private static Path dir;

    public static void start(Path oracleDir) {
        if (started) return;
        started = true;
        dir = oracleDir;
        int port = Integer.getInteger("basalt.oracle.ctl.port", 25789);
        Thread t = new Thread(() -> serve(port), "basalt-oracle-ctl");
        t.setDaemon(true);
        t.start();
    }

    private static void serve(int port) {
        try (ServerSocket ss = new ServerSocket(port, 4, InetAddress.getLoopbackAddress())) {
            System.out.println("[basalt-oracle] control channel on 127.0.0.1:" + port);
            while (true) {
                Socket s = ss.accept();
                s.setTcpNoDelay(true);
                Conn c = new Conn(s);
                Thread r = new Thread(() -> read(c), "basalt-oracle-ctl-conn");
                r.setDaemon(true);
                r.start();
            }
        } catch (Throwable e) { e.printStackTrace(); }
    }

    private static void read(Conn c) {
        try (BufferedReader br = new BufferedReader(new InputStreamReader(c.sock.getInputStream(), StandardCharsets.UTF_8))) {
            String line;
            while ((line = br.readLine()) != null) {
                line = line.trim();
                if (line.isEmpty()) continue;
                try { QUEUE.add(new Cmd(JsonParser.parseString(line).getAsJsonObject(), c)); }
                catch (Exception e) { JsonObject o = new JsonObject(); o.addProperty("ok", false); o.addProperty("error", "bad json: " + e.getMessage()); c.send(o); }
            }
        } catch (IOException ignored) {}
    }

    // ------------------------------------------------------------------------------ tick hook

    public static void onTick(Minecraft mc) {
        tick++;
        try {
            for (Iterator<Object[]> it = RELEASES.iterator(); it.hasNext(); ) {
                Object[] r = it.next();
                if ((long) r[1] <= tick) { ((KeyMapping) r[0]).setDown(false); it.remove(); }
            }
            if (forceGrab) grab(mc);
            Cmd c;
            while ((c = QUEUE.poll()) != null) LOCAL.add(c);
            while (true) {
                if (blocking != null) {
                    if (!finishBlocking(mc)) { Anim.onTick(mc); return; }
                }
                Cmd next = LOCAL.poll();
                if (next == null) { Anim.onTick(mc); return; }
                run(mc, next);
            }
        } catch (Throwable e) { e.printStackTrace(); }
    }

    private static JsonObject reply(Cmd c, boolean ok) {
        JsonObject o = new JsonObject();
        if (c.req.has("id")) o.add("id", c.req.get("id"));
        o.addProperty("ok", ok);
        o.addProperty("tick", tick);
        return o;
    }

    private static void run(Minecraft mc, Cmd c) {
        JsonObject o;
        try {
            String op = c.req.get("op").getAsString();
            o = reply(c, true);
            switch (op) {
                case "ping" -> o.addProperty("pong", true);
                case "wait" -> { blocking = c; blockUntil = tick + Math.max(0, geti(c.req, "ticks", 1)); blockDeadline = Long.MAX_VALUE; return; }
                case "wait_join" -> { blocking = c; blockUntil = -1; blockDeadline = tick + geti(c.req, "timeout", 1200); return; }
                case "wait_screen" -> { blocking = c; blockUntil = -2; blockDeadline = tick + geti(c.req, "timeout", 200); return; }
                case "connect" -> connect(mc, c.req.get("addr").getAsString());
                case "disconnect" -> mc.disconnectWithProgressScreen();
                case "move" -> {
                    for (String k : new String[] { "forward", "back", "left", "right" })
                        if (c.req.has(k)) hold(mc, k, c.req.get(k).getAsBoolean(), false);
                }
                case "jump", "sneak", "sprint", "attack", "use" -> keyOp(mc, op, c.req);
                case "key" -> keyOp(mc, c.req.get("name").getAsString(), c.req);
                case "hotbar" -> {
                    int n = c.req.get("slot").getAsInt();
                    KeyMapping km = mc.options.keyHotbarSlots[n];
                    tap(km, 1);
                }
                case "look" -> look(mc, c.req);
                case "inventory" -> {
                    String a = c.req.has("action") ? c.req.get("action").getAsString() : "toggle";
                    boolean open = mc.gui.screen() instanceof AbstractContainerScreen<?>;
                    if (a.equals("open") && !open || a.equals("toggle") && !open) tap(mc.options.keyInventory, 1);
                    else if (a.equals("close") && open || a.equals("toggle") && open) closeScreen(mc);
                }
                case "escape" -> closeScreen(mc);
                case "click" -> click(mc, c.req, o);
                case "drag" -> drag(mc, c.req);
                case "chat" -> mc.player.connection.sendChat(c.req.get("text").getAsString());
                case "command" -> mc.player.connection.sendCommand(c.req.get("text").getAsString());
                case "grab" -> { forceGrab = !c.req.has("on") || c.req.get("on").getAsBoolean(); }
                case "log" -> {
                    if (c.req.has("file")) PacketLog.open(Paths.get(c.req.get("file").getAsString()));
                    else if (c.req.has("close")) PacketLog.close();
                    o.addProperty("file", PacketLog.file());
                    o.addProperty("packets", PacketLog.count());
                }
                case "mark" -> PacketLog.mark(c.req.get("label").getAsString());
                case "flush" -> { PacketLog.flush(); o.addProperty("packets", PacketLog.count()); }
                case "query" -> state(mc, o);
                case "bones" -> Anim.bonesOp(mc, c.req, o);
                case "fp" -> Anim.fpOp(c.req, o);
                case "frames" -> Anim.framesOp(c.req, o);
                case "hud" -> { boolean on = c.req.get("on").getAsBoolean(); if (mc.gui.hud.isHidden() == on) mc.gui.hud.toggle(); }
                case "camera" -> mc.options.setCameraType(net.minecraft.client.CameraType.valueOf(c.req.get("type").getAsString()));
                case "bob" -> mc.options.bobView().set(c.req.get("on").getAsBoolean());
                case "release_all" -> { KeyMapping.releaseAll(); RELEASES.clear(); }
                case "quit" -> { c.conn.send(o); PacketLog.close(); mc.stop(); return; }
                default -> { o.addProperty("ok", false); o.addProperty("error", "unknown op " + op); }
            }
        } catch (Throwable e) {
            o = reply(c, false);
            StringWriter sw = new StringWriter(); e.printStackTrace(new PrintWriter(sw));
            o.addProperty("error", sw.toString());
        }
        c.conn.send(o);
    }

    private static boolean finishBlocking(Minecraft mc) {
        Cmd c = blocking;
        boolean done; boolean timeout = false;
        if (blockUntil >= 0) done = tick >= blockUntil;
        else if (blockUntil == -1) { done = inWorld(mc) && mc.gui.screen() == null && mc.getConnection() != null; timeout = !done && tick >= blockDeadline; }
        else {
            String want = c.req.get("screen").getAsString();
            Screen s = mc.gui.screen();
            String have = s == null ? "none" : s.getClass().getSimpleName();
            done = have.equals(want); timeout = !done && tick >= blockDeadline;
        }
        if (!done && !timeout) return false;
        blocking = null;
        JsonObject o = reply(c, !timeout);
        if (timeout) o.addProperty("error", "timeout waiting (" + c.req.get("op").getAsString() + ")");
        c.conn.send(o);
        return true;
    }

    // ------------------------------------------------------------------------------ helpers

    private static int geti(JsonObject o, String k, int d) { return o.has(k) ? o.get(k).getAsInt() : d; }

    private static boolean inWorld(Minecraft mc) { return mc.player != null && mc.level != null; }

    private static void connect(Minecraft mc, String addr) {
        ServerAddress sa = ServerAddress.parseString(addr);
        ServerData sd = new ServerData("oracle", addr, ServerData.Type.OTHER);
        ConnectScreen.startConnecting(new TitleScreen(), mc, sa, sd, false, null);
    }

    private static KeyMapping mapping(Minecraft mc, String name) {
        var o = mc.options;
        return switch (name) {
            case "forward", "up" -> o.keyUp;
            case "back", "down" -> o.keyDown;
            case "left" -> o.keyLeft;
            case "right" -> o.keyRight;
            case "jump" -> o.keyJump;
            case "sneak", "shift" -> o.keyShift;
            case "sprint" -> o.keySprint;
            case "attack" -> o.keyAttack;
            case "use" -> o.keyUse;
            case "inventory" -> o.keyInventory;
            case "drop" -> o.keyDrop;
            case "swap", "swap_offhand" -> o.keySwapOffhand;
            case "pick" -> o.keyPickItem;
            default -> {
                KeyMapping km = KeyMapping.get(name);
                if (km == null) throw new IllegalArgumentException("unknown key '" + name + "'");
                yield km;
            }
        };
    }

    /** Press/release/tap/hold a key mapping (a "click" is counted on press so consumeClick works). */
    private static void hold(Minecraft mc, String name, boolean down, boolean click) {
        KeyMapping km = mapping(mc, name);
        km.setDown(down);
        if (down && click) KeyMapping.click(km.getDefaultKey());
    }

    private static void tap(KeyMapping km, int ticks) {
        km.setDown(true);
        KeyMapping.click(km.getDefaultKey());
        RELEASES.add(new Object[] { km, tick + ticks });
    }

    /** {"op":"attack","action":"press|hold|release|tap","ticks":1}; press = click + down, release = up. */
    private static void keyOp(Minecraft mc, String name, JsonObject req) {
        String a = req.has("action") ? req.get("action").getAsString() : "tap";
        KeyMapping km = mapping(mc, name);
        switch (a) {
            case "press", "hold" -> { km.setDown(true); KeyMapping.click(km.getDefaultKey()); }
            case "release" -> km.setDown(false);
            case "tap" -> tap(km, geti(req, "ticks", 1));
            default -> throw new IllegalArgumentException("action " + a);
        }
    }

    private static void closeScreen(Minecraft mc) {
        Screen s = mc.gui.screen();
        if (s != null) s.onClose();
        else mc.pauseGame(false);
    }

    private static void look(Minecraft mc, JsonObject req) {
        var p = mc.player;
        boolean rel = req.has("relative") && req.get("relative").getAsBoolean();
        if (req.has("yaw")) p.setYRot((rel ? p.getYRot() : 0f) + req.get("yaw").getAsFloat());
        if (req.has("pitch")) {
            float v = (rel ? p.getXRot() : 0f) + req.get("pitch").getAsFloat();
            p.setXRot(Math.max(-90f, Math.min(90f, v)));
        }
        p.yRotO = p.getYRot(); p.xRotO = p.getXRot();
    }

    private static void grab(Minecraft mc) {
        try {
            if (inWorld(mc) && mc.gui.screen() == null && !mc.mouseHandler.isMouseGrabbed()) {
                var f = mc.mouseHandler.getClass().getDeclaredField("mouseGrabbed");
                f.setAccessible(true);
                f.setBoolean(mc.mouseHandler, true);
            }
        } catch (Throwable ignored) {}
    }

    private static Method SLOT_CLICKED;

    private static ContainerInput mode(int id) {
        for (ContainerInput m : ContainerInput.values()) if (m.id() == id) return m;
        throw new IllegalArgumentException("click mode " + id);
    }

    private static void slotClicked(Minecraft mc, int slot, int button, int mode) throws Exception {
        Screen s = mc.gui.screen();
        if (!(s instanceof AbstractContainerScreen<?> acs)) throw new IllegalStateException("no container screen open (screen=" + (s == null ? "none" : s.getClass().getSimpleName()) + ")");
        if (SLOT_CLICKED == null) {
            SLOT_CLICKED = AbstractContainerScreen.class.getDeclaredMethod("slotClicked", Slot.class, int.class, int.class, ContainerInput.class);
            SLOT_CLICKED.setAccessible(true);
        }
        AbstractContainerMenu menu = acs.getMenu();
        Slot sl = slot >= 0 && slot < menu.slots.size() ? menu.slots.get(slot) : null;
        SLOT_CLICKED.invoke(acs, sl, slot, button, mode(mode));
    }

    /** Through the screen's own slotClicked, i.e. the code path a mouse click on a slot ends in. */
    private static void click(Minecraft mc, JsonObject req, JsonObject o) throws Exception {
        slotClicked(mc, req.get("slot").getAsInt(), geti(req, "button", 0), geti(req, "mode", 0));
    }

    /** Quick-craft drag as the screen performs it: start (-999), one add per slot, end (-999). */
    private static void drag(Minecraft mc, JsonObject req) throws Exception {
        int type = geti(req, "button", 0);   // 0 = left (split), 1 = right (one each), 2 = middle (clone)
        slotClicked(mc, -999, AbstractContainerMenu.getQuickcraftMask(0, type), 5);
        for (JsonElement e : req.getAsJsonArray("slots")) slotClicked(mc, e.getAsInt(), AbstractContainerMenu.getQuickcraftMask(1, type), 5);
        slotClicked(mc, -999, AbstractContainerMenu.getQuickcraftMask(2, type), 5);
    }

    // ------------------------------------------------------------------------------ state

    static JsonObject stack(ItemStack s) {
        JsonObject o = new JsonObject();
        if (s == null || s.isEmpty()) { o.addProperty("id", "minecraft:air"); o.addProperty("count", 0); return o; }
        o.addProperty("id", BuiltInRegistries.ITEM.getKey(s.getItem()).toString());
        o.addProperty("count", s.getCount());
        if (s.isDamaged()) o.addProperty("damage", s.getDamageValue());
        return o;
    }

    private static void state(Minecraft mc, JsonObject o) {
        o.addProperty("in_world", inWorld(mc));
        Screen s = mc.gui.screen();
        o.addProperty("screen", s == null ? "none" : s.getClass().getSimpleName());
        o.addProperty("overlay", mc.gui.overlay() == null ? "none" : mc.gui.overlay().getClass().getSimpleName());
        o.addProperty("packets", PacketLog.count());
        if (!inWorld(mc)) return;
        var p = mc.player;
        JsonArray pos = new JsonArray();
        pos.add(p.getX()); pos.add(p.getY()); pos.add(p.getZ());
        o.add("pos", pos);
        o.addProperty("yaw", p.getYRot());
        o.addProperty("pitch", p.getXRot());
        o.addProperty("on_ground", p.onGround());
        o.addProperty("sneaking", p.isCrouching());
        o.addProperty("sprinting", p.isSprinting());
        o.addProperty("selected", p.getInventory().getSelectedSlot());
        o.addProperty("health", p.getHealth());
        o.addProperty("food", p.getFoodData().getFoodLevel());
        o.addProperty("saturation", p.getFoodData().getSaturationLevel());
        o.add("held", stack(p.getMainHandItem()));
        HitResult hr = mc.hitResult;
        if (hr != null) {
            JsonObject h = new JsonObject();
            h.addProperty("type", hr.getType().toString());
            if (hr instanceof BlockHitResult bhr && hr.getType() == HitResult.Type.BLOCK) {
                var bp = bhr.getBlockPos();
                JsonArray a = new JsonArray(); a.add(bp.getX()); a.add(bp.getY()); a.add(bp.getZ());
                h.add("pos", a);
                h.addProperty("face", bhr.getDirection().toString());
                h.addProperty("block", mc.level.getBlockState(bp).toString());
            }
            o.add("hit", h);
        }
        AbstractContainerMenu m = p.containerMenu;
        JsonObject mo = new JsonObject();
        mo.addProperty("id", m.containerId);
        String mtype = "player";
        try { if (m.getType() != null) mtype = String.valueOf(BuiltInRegistries.MENU.getKey(m.getType())); } catch (UnsupportedOperationException e) { /* the inventory menu has no type */ }
        mo.addProperty("type", mtype);
        mo.addProperty("state_id", m.getStateId());
        mo.add("carried", stack(m.getCarried()));
        JsonArray slots = new JsonArray();
        for (Slot sl : m.slots) slots.add(stack(sl.getItem()));
        mo.add("slots", slots);
        o.add("menu", mo);
    }
}
