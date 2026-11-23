package basalt.oracle.mixin;

import basalt.oracle.PacketLog;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFutureListener;
import net.minecraft.network.Connection;
import net.minecraft.network.protocol.Packet;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Test-only: notes every packet handed to the connection (tick stamp + class) before it is encoded. */
@Mixin(Connection.class)
public abstract class ConnectionMixin {
    @Shadow private Channel channel;

    @Inject(method = "send(Lnet/minecraft/network/protocol/Packet;Lio/netty/channel/ChannelFutureListener;Z)V", at = @At("HEAD"))
    private void basalt$send(Packet<?> packet, ChannelFutureListener listener, boolean flush, CallbackInfo ci) {
        PacketLog.onSend(channel, packet);
    }
}
