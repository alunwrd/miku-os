var TextInput = {
    AlignLeft: 1, AlignRight: 2, AlignHCenter: 4,
    AlignTop: 32, AlignBottom: 64, AlignVCenter: 128,
    Normal: 0, NoEcho: 1, Password: 2, PasswordEchoOnEdit: 3,
    CursorBetweenCharacters: 0, CursorOnCharacter: 1,
};
var TextField = TextInput;
var Flickable = {
    StopAtBounds: 0, DragOverBounds: 1, OvershootBounds: 2, DragAndOvershootBounds: 3,
    AutoFlickDirection: 0, HorizontalFlick: 1, VerticalFlick: 2, HorizontalAndVerticalFlick: 3,
};
// quickshell's layer-shell and panel enums (`mayland/`)
var WlrLayer = { Background: 0, Bottom: 1, Top: 2, Overlay: 3 };
var WlrKeyboardFocus = { None: 0, Exclusive: 1, OnDemand: 2 };
var ExclusionMode = { Normal: 0, Ignore: 1, Auto: 2 };

var Text = {
    AlignLeft: 1, AlignRight: 2, AlignHCenter: 4, AlignJustify: 8,
    AlignTop: 32, AlignBottom: 64, AlignVCenter: 128,
    ElideLeft: 0, ElideRight: 1, ElideMiddle: 2, ElideNone: 3,
    NoWrap: 0, WordWrap: 1, WrapAnywhere: 3, Wrap: 4,
    PlainText: 0, RichText: 1, AutoText: 2, StyledText: 4, MarkdownText: 3,
    Normal: 0, Outline: 1, Raised: 2, Sunken: 3,
    QtRendering: 0, NativeRendering: 1, CurveRendering: 2,
    ProportionalHeight: 0, FixedHeight: 1,
    FixedSize: 0, HorizontalFit: 1, VerticalFit: 2, Fit: 3,
};

var SystemClock = { Hours: 0, Minutes: 1, Seconds: 2 };
var Animation = { Infinite: -1 };
var ListView = {
    Horizontal: 1, Vertical: 2,
    Beginning: 0, Center: 1, End: 2, Visible: 3, Contain: 4, SnapPosition: 5,
    NoSnap: 0, SnapToItem: 1, SnapOneItem: 2,
    NoHighlightRange: 0, ApplyRange: 1, StrictlyEnforceRange: 2,
};
var Image = {
    Stretch: 0, PreserveAspectFit: 1, PreserveAspectCrop: 2, Tile: 3, TileVertically: 4,
    TileHorizontally: 5, Pad: 6,
    Null: 0, Ready: 1, Loading: 2, Error: 3,
};
var Loader = { Null: 0, Ready: 1, Loading: 2, Error: 3 };
var Component = { Null: 0, Ready: 1, Loading: 2, Error: 3 };

var Font = {
    Thin: 100, ExtraLight: 200, Light: 300, Normal: 400, Medium: 500,
    DemiBold: 600, Bold: 700, ExtraBold: 800, Black: 900,
    PreferDefaultHinting: 0, PreferNoHinting: 1, PreferVerticalHinting: 2, PreferFullHinting: 3,
    MixedCase: 0, AllUppercase: 1, AllLowercase: 2, SmallCaps: 3, Capitalize: 4,
};

var Easing = {
    Linear: 0, InQuad: 1, OutQuad: 2, InOutQuad: 3, OutInQuad: 4,
    InCubic: 5, OutCubic: 6, InOutCubic: 7, OutInCubic: 8,
    InQuart: 9, OutQuart: 10, InOutQuart: 11, OutInQuart: 12,
    InQuint: 13, OutQuint: 14, InOutQuint: 15, OutInQuint: 16,
    InSine: 17, OutSine: 18, InOutSine: 19, OutInSine: 20,
    InExpo: 21, OutExpo: 22, InOutExpo: 23, OutInExpo: 24,
    InCirc: 25, OutCirc: 26, InOutCirc: 27, OutInCirc: 28,
    InElastic: 29, OutElastic: 30, InOutElastic: 31, OutInElastic: 32,
    InBack: 33, OutBack: 34, InOutBack: 35, OutInBack: 36,
    InBounce: 37, OutBounce: 38, InOutBounce: 39, OutInBounce: 40,
    BezierSpline: 45,
};

var RotationAnimation = { Numerical: 0, Clockwise: 1, Counterclockwise: 2, Shortest: 3 };
// `transformOrigin: Item.Center`
var Item = {
    TopLeft: 0, Top: 1, TopRight: 2, Left: 3, Center: 4, Right: 5, BottomLeft: 6, Bottom: 7, BottomRight: 8,
};
var Drag = { XAxis: 1, YAxis: 2, XAndYAxis: 3, None: 0, Automatic: 1, Internal: 2 };
var Binding = { RestoreNone: 0, RestoreBinding: 1, RestoreValue: 2, RestoreBindingOrValue: 3 };
var Grid = { LeftToRight: 0, TopToBottom: 1, AlignLeft: 1, AlignRight: 2, AlignHCenter: 4, AlignTop: 32, AlignBottom: 64, AlignVCenter: 128 };
var GridLayout = { LeftToRight: 0, TopToBottom: 1 };
var Flow = { LeftToRight: 0, TopToBottom: 1 };
var FontLoader = { Null: 0, Ready: 1, Loading: 2, Error: 3 };
var Gradient = { Vertical: 2, Horizontal: 1 };
var TextEdit = Object.assign({ AutoText: 2, PlainText: 0, RichText: 1, MarkdownText: 3,
    NoWrap: 0, WordWrap: 1, WrapAnywhere: 3, Wrap: 4 }, TextInput);
var TextArea = TextEdit;
var Canvas = { Image: 0, FramebufferObject: 1, Immediate: 0, Threaded: 1, Cooperative: 2 };
// QtQuick.Controls' own
var ScrollBar = { AsNeeded: 0, AlwaysOff: 1, AlwaysOn: 2, NoSnap: 0, SnapAlways: 1, SnapOnRelease: 2 };
var AbstractButton = { IconOnly: 0, TextOnly: 1, TextBesideIcon: 2, TextUnderIcon: 3 };
var Button = AbstractButton;
var SwipeDelegate = {};
// quickshell's services' (`services/mod.rs`: Mpris and UPower have nothing
// behind them on MikuOS, but a file still compares with their values)
var MprisPlaybackState = { Stopped: 0, Playing: 1, Paused: 2 };
var MprisLoopState = { None: 0, Track: 1, Playlist: 2 };
var UPowerDeviceState = { Unknown: 0, Charging: 1, Discharging: 2, Empty: 3, FullyCharged: 4, PendingCharge: 5,
    PendingDischarge: 6 };
var UPowerDeviceType = {
    Unknown: 0, LinePower: 1, Battery: 2, Ups: 3, Monitor: 4, Mouse: 5, Keyboard: 6, Pda: 7, Phone: 8,
    MediaPlayer: 9, Tablet: 10, Computer: 11, GamingInput: 12, Pen: 13, Touchpad: 14, Modem: 15, Network: 16,
    Headset: 17, Speakers: 18, Headphones: 19, Video: 20, OtherAudio: 21, RemoteControl: 22, Printer: 23,
    Scanner: 24, Camera: 25, Wearable: 26, Toy: 27, BluetoothGeneric: 28,
};
var Shape = { UnknownRenderer: 0, GeometryRenderer: 1, NvprRenderer: 2, SoftwareRenderer: 3, CurveRenderer: 4,
              BoundingRectContains: 0, FillContains: 1, NoResize: 0, PreserveAspectFit: 1, PreserveAspectCrop: 2, Stretch: 3,
              Null: 0, Ready: 1, Processing: 2 };
var ShapePath = { OddEvenFill: 0, WindingFill: 1, MiterJoin: 0, BevelJoin: 64, RoundJoin: 128,
                  FlatCap: 0, SquareCap: 16, RoundCap: 32, SolidLine: 1, DashLine: 2,
                  PathLinear: 1, PathQuadratic: 2, PathConvex: 4, PathFillOnRight: 8, PathSolid: 16,
                  PathNonIntersecting: 32, PathNonOverlappingControlPointTriangles: 64 };
// quickshell's Region: how a region meets the one it is in, and its shape
var Intersection = { Combine: 0, Subtract: 1, Intersect: 2, Xor: 3 };
var RegionShape = { Rect: 0, Ellipse: 1 };
