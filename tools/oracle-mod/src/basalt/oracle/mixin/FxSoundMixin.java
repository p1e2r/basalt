package basalt.oracle.mixin;

import basalt.oracle.PacketLog;
import com.google.gson.JsonObject;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.sounds.SoundEvent;
import net.minecraft.sounds.SoundSource;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Test-only: logs the sounds the client plays locally as {"tick","fx":"sound",...} lines. */
@Mixin(ClientLevel.class)
public abstract class FxSoundMixin {
    @Inject(method = "playSound(DDDLnet/minecraft/sounds/SoundEvent;Lnet/minecraft/sounds/SoundSource;FFZJ)V", at = @At("HEAD"))
    private void basalt$sound(double x, double y, double z, SoundEvent e, SoundSource s, float vol, float pitch, boolean dist, long seed, CallbackInfo ci) {
        JsonObject o = new JsonObject();
        o.addProperty("fx", "sound");
        o.addProperty("name", e.location().toString());
        o.addProperty("cat", s.getName());
        o.addProperty("x", x); o.addProperty("y", y); o.addProperty("z", z);
        o.addProperty("vol", vol); o.addProperty("pitch", pitch);
        PacketLog.fx(o);
    }
}
